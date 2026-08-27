# CNMPlayer 代码审查报告

> 审查范围：主 crate（`src/`，约 2 万行）+ `ncm-api-rs` 子 crate + `cargo clippy` 静态检查结果
>
> 整体评价：架构清晰（compio 异步 + rodio 播放 + ratatui 双 UI），release profile 已配置 LTO / codegen-units=1 / strip。以下按优先级列出发现的问题。

> GLM5.3-flash,2026-08-28
---

## 一、正确性 / 可靠性问题（建议优先修）

### 1. `src/app/streaming.rs` — 丢失唤醒竞态（流式播放卡顿根因之一）

`download_streaming` 中，`downloaded` 的更新和 `notify_all()` 发生在**释放 `file_lock` 之后**：

```rust
{
    let _guard = state.file_lock.lock().unwrap();
    cursor.write_all(chunk).await.0?;   // 持锁写
}                                        // 先放锁
state.downloaded.store(downloaded, ...); // 再无锁 store + notify
state.condvar.notify_all();
```

而读者 `wait_for_position` 是「持锁检查 → 无锁窗口 → 进入 wait」。竞态窗口内 notify 会丢失，读者白等满 1 秒超时（`wait_timeout` 兜底），表现为流式播放周期性卡顿。

**修复**：`downloaded` 的 store 必须在持有 `file_lock` 期间完成（notify 可以在锁外）。

### 2. `src/app/streaming.rs` — 跨 await 持有 std Mutex（clippy 报了 3 处）

- L265-267：持锁 `OpenOptions...open().await`
- L285-287：持锁 `write_all(chunk).await`（网络 chunk 落盘期间，音频线程的 read/seek 全部拿不到锁 → 磁盘一慢音频就卡）
- L298-300：持锁 `file.close().await` + `rename().await`

这个锁同时承担「文件操作互斥」和「Condvar 等待」两个职责。建议拆开：文件读写各持自己的句柄（reader 独占读、writer 独占写同一文件是允许的），共享状态只剩原子量 + 通知专用小 Mutex。

### 3. `src/app/streaming.rs` — StreamingReader 无法取消

`_writer: JoinHandle` 只为持有、从不 abort。切歌时旧下载任务会**继续跑完整个文件**（浪费带宽），完成后 `rename` 还会把 Drop 时刚删掉的 `.part`/缓存文件「复活」。

**修复**：需要一个 `AtomicBool` 取消标志，每 chunk 检查，Drop 时置位。

### 4. `src/app/api.rs` `fetch_cover_bytes` — 吞掉流错误

```rust
while let Some(Ok(chunk)) = stream.next().await { ... }
```

网络中途出错时 Err 被当作正常结束，**截断的图片会被写入磁盘缓存并长期使用**。应显式处理 `Some(Err(e))` 返回错误。

### 5. `ncm-api-rs/src/crypto.rs` — 每次 weapi 请求重新解析 RSA 公钥

`rsa_encrypt_no_padding` 每次调用都 base64 解码 DER + `from_public_key_der` + `to_odd`。

**修复**：用 `static PUBLIC_KEY: LazyLock<RsaPublicKey>` 缓存（weapi 虽不是默认路径，但触发时是纯浪费）。

### 6. `src/tmplayer/app/event_loop.rs:655` — async fn 里 `std::thread::sleep`

全屏模式的整个主循环跑在 compio 异步任务里，帧尾用 `thread::sleep` 限帧，会阻塞所在 worker 线程一整个帧间隔，同一线程上的下载任务、封面/歌词 fetch worker、host bridge tick 全部停摆。

**修复**：改 `compio::time::sleep(...).await`。

---

## 二、性能优化空间

### 7. `src/app/player.rs` `EqSource::next()` — 每个采样点构造数组（音频实时路径）

```rust
let current = self.params.load_db_x10();  // 每个样本 std::array::from_fn 构造 [i32;10]
if current != self.last_db_x10 { ... }
```

44.1kHz × 2ch ≈ 8.8 万次/秒的原子读取 + 数组构造 + 比较。

**修复**：改为一个 `AtomicU32` 版本号：先读版本号，变了才重建系数。顺带修复 `load_db()`/`load_db_x10()` 在更新分支里重复读两遍原子的问题。EQ 关闭（全部 0dB）时也可直接旁路 biquad 链，省 10 次 biquad/样本。

### 8. `src/app/mod.rs` `play_queue_index` — 切歌前 await 网络

- L2997 `refresh_now_playing_like_state().await` 在音频启动前做一次 `song_like_check` 网络往返。本地 `liked_song_ids.contains` 已经先赋了值，远程校验应放后台任务。
- L2998 `persist_playback_memory()` 同步 `fs::write` 也在关键路径上（会话文件小，问题不大，但可挪后台）。

### 9. 封面 bytes 大克隆

`apply_cover_fetch_result` 里 `bytes.clone()`（数百 KB 级）存两份（`now_playing` + queue slot）。`PlaybackTrack.cover` 改 `Arc<Vec<u8>>` 即可共享。同样 `FullscreenTrackSeed.cover` 在 `build_fullscreen_bootstrap`、`AppFullscreenBridge::snapshot` 里被反复整份克隆。

### 10. `src/app/mod.rs` `main_spectrum_braille`（L2087）

循环里调了 10 次 `self.cava_bars()`（每次都取 watch 值）。提出循环取一次即可。`cava.rs` 的 `latest_bars()` 每帧 clone 两个 Vec + 两次锁，可换成 `[f32; 96]` 定长数组。

### 11. `src/app/streaming.rs` — 每 chunk 一次 `progress_tx.send` + `notify_all`

高带宽时每秒上千次唤醒。对 watch channel 来说 send 本身便宜，但 `notify_all` 可按字节阈值（如每 64KB）节流。

### 12. `src/main.rs` `run_app` — 空闲时每秒全量重绘

`sleep(1s)` 分支无条件触发 draw，重新计算全部 UI（ASCII 封面、braille、hit rects 重建）。虽然 ratatui 会 diff 输出，但重算成本仍在。已有 `sync_on_change` 机制，可给 tick 分支加 dirty 判断。

---

## 三、结构 / 可维护性

### 13. `src/app/mod.rs` 7377 行

`impl App` 就有 4400+ 行。至少可拆出：

- `parse_*` JSON 工具家族（~L6340-7100，800 行）→ `app/parse.rs`
- keybind 匹配/规范化（~L7160-7360）→ `app/keybind.rs`
- fullscreen bridge/snapshot（~L4550-4910）→ `app/fullscreen.rs`
- 登录流程（~L5140-5310）→ `app/login.rs`

### 14. 双份配置/主题/资源模块 + 手写字段映射

- `src/data/{config,theme_loader,assets}.rs` 与 `src/tmplayer/data/*` 基本是复制（diff 主要只是路径和命名差异）
- 配置同步靠 `host_config_sync_from_app` / `apply_host_config_sync`（`event_loop.rs` L80-165）+ `AppFullscreenBridge` 里**第三份** enum match，30+ 字段逐一转换。加一个配置项要改 4-5 处。

**建议**：把两个 config 合并为一个共享类型（tmplayer 直接引用主 config），enum 转换用 `From` 实现收拢到一处。

### 15. clippy：139 个警告

其中 93 个 `collapsible_if`、若干 `map(unit)`、`needless_borrow` 等，`cargo clippy --fix` 可自动清 123 个。

### 16. 小问题

- `main.rs:196` `expect(&msg)` → `expect(msg)`；函数名 `stroage_or_abort` 拼写（storage）
- `main.rs:167-172` `Picker::from_query_stdio()` 结果被注释丢弃——终端探测 syscall 白做，可直接删
- `ui/search.rs:173` `if focused { row_style } else { row_style }` 两分支相同
- `app/player.rs` `play_from_file(&PathBuf)` → `&Path`
- `cava.rs` `temp_cfg_path()` 写死 `/tmp`，建议 `std::env::temp_dir()`

---

## 建议的动手顺序

1. **#1 / #2 / #3**（streaming 稳定性，直接影响听感）
2. **#4 / #5**（数据损坏 / 浪费）
3. **#7 / #8**（音频路径与切歌延迟）
4. **#13 / #14**（拆文件、合并配置，工作量大但收益高）
