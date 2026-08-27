# cnmplayer 代码审查与优化建议

> 审查日期：代码通读（main / app / streaming / player / tmplayer / ui / ncm-api）
> 状态：`cargo check` 通过，无编译警告（仅非 Linux 平台的 MPRIS dead_code 警告）
> Deepseek-v4-flash,2026-08-27
---

## 🔴 高优先级：网络请求阻塞事件循环（切歌/进全屏会卡 UI）

`run_app` 主循环在 `select_biased!` 中 await 事件处理闭包 `f(app)` 期间既不渲染也不处理输入。以下操作把网络请求直接 await 在了事件循环上，网络慢时 UI 冻结数秒。

### 1. `play_queue_index` — 切歌时串行 await 两次网络请求

- 位置：`src/app/mod.rs:2979`
- 问题：
  - `refresh_now_playing_like_state().await`（:2992，`song_like_check` 接口）
  - `song_stream_url_with_quality().await`（:3042，取流 URL）
  - 两次都是网络往返，期间 UI 完全冻结。
- 建议：复用现有 cover/lyric 后台任务模式（`loop_cover_fetch` / `loop_lyric_fetch` + mpsc 结果队列）：把取流 URL 放到后台任务，先切换 UI 状态并显示「正在缓冲」，URL 就绪后再启动播放。

### 2. `build_fullscreen_bootstrap` — Ctrl+F 进全屏串行拉取多个接口

- 位置：`src/app/mod.rs:4708`
- 问题：`song_detail` → 封面下载 → `lyric` 等请求**串行** await，全部完成才进全屏。
- 建议：`futures::join!` 并行化；或先进全屏再后台补齐封面/歌词（bootstrap 已带 `now_playing.cover/lyrics`，部分请求是重复的）。

### 3. `load_author_detail` — 打开作者页 4 个串行请求

- 位置：`src/app/mod.rs:~3810`
- 问题：`artist_detail` / `artist_desc` / `artist_top_song` / `artist_album` 串行 await，页面加载慢 ~4 倍。
- 建议：`futures::join!` 并行。

### 4. `load_home_sidebar_playlists` / `refresh_vip_audio_access`

- 位置：`src/app/mod.rs`
- 问题：各含 2~3 个串行请求。
- 建议：并行化。

### 5. 启动流程 — Loading 动画在启动场景是死代码

- 位置：`src/main.rs:87-92`
- 问题：`App::new` 里 await 完登录恢复 + 推荐加载之后才 `init_terminal()`，因此 `Page::Loading` / `startup_loading_progress` 动画在启动时根本显示不出来。
- 建议：要么接受现状，要么把网络加载移到进入渲染循环之后再触发。

---

## 🟡 中优先级：每帧/每次按键的冗余计算

### 6. `fullscreen_metadata_signature` — 每帧重 hash 整个播放队列

- 位置：`src/app/mod.rs:2277`
- 问题：全屏事件循环**每帧**调用 `metadata_signature()`，其内部遍历整个 `playback_queue`（每首歌 song_id + duration）重新 hash。队列有几百上千首歌时纯属浪费。
- 建议：仅在 `playback_queue` / `current_index` 变化时重算并缓存签名；或改用单调递增的 dirty 计数器。

### 7. `draw_home_sidebar` — 每帧 clone 整个播放列表

- 位置：`src/ui/home.rs:366-367`
- 问题：`created_playlists.clone()` + `collected_playlists.clone()`（最多 100+100 个含多个 String 的结构体），每帧执行。
- 建议：下面函数签名已是 `&[HomeSidebarPlaylist]`，直接传 `&app.home_sidebar.created_playlists` 借用即可。

### 8. `draw_tiles` — 每个 tile 每帧 clone 标题

- 位置：`src/ui/home.rs:188`
- 问题：`(tile.title.clone(), tile.subtitle.clone())` 每帧每 tile 两次分配。
- 建议：用 `tile.title.as_str()` 借用（`Span::styled` 接受 `&str`）。

### 9. `keybind_action_from_event` — 每次按键做 ~19 次带分配的归一化匹配

- 位置：`src/app/mod.rs:2511`
- 问题：每次按键遍历 19 个绑定，每个都走 `normalize_keybind_text` + `key_event_to_keybind_text`，每次分配多个 String。
- 建议：用 `LazyLock` 预归一化配置绑定；事件侧只归一化一次。

### 10. `main_spectrum_braille` — 循环内重复取谱数据

- 位置：`src/app/mod.rs:2087-2090`
- 问题：循环内调用 `self.cava_bars()` 10 次（每次 2 次 mutex 加锁 + vec 复制）。
- 建议：先取一次 `let bars = self.cava_bars();` 再循环索引。

---

## 🟡 中优先级：流式下载与缓存

### 11. `download_streaming` — 每个网络 chunk 都 flush

- 位置：`src/app/streaming.rs:287`
- 问题：每个 chunk `cursor.flush().await` 一次 syscall。
- 建议：`write_all` 已落盘，flush 只在下载结束或每 N 个 chunk 做一次。

### 12. `StreamingReader::read` — 一次 read 多次拿锁 + SeqCst

- 位置：`src/app/streaming.rs:196-210`
- 问题：取 pos 加一次锁、`wait_for_position` 再加、真正 read 再加；`downloaded`/`done` 原子量用 `Ordering::SeqCst`。
- 建议：合并为单次持锁；原子量改 `Acquire/Release`。

### 13. MPRIS 封面落盘 — 每次切歌全目录扫描

- 位置：`src/app/mpris_bridge.rs:246`
- 问题：每次持久化封面后跑一次完整 `cleanup_cache_dir` 扫描排序。
- 建议：限频（如每 5 分钟一次）或仅启动时清理。

---

## 🟢 低优先级 / 代码卫生

### 14. `generate_random_cover_ascii` — 死代码/重复实现

- 位置：`src/tmplayer/ui/panels/info_panel.rs:660`
- 问题：与 `fill_ascii` 完全重复，`seed` 参数已死。
- 建议：删除，改用 `fill_ascii`。

### 15. `App::new` 中 `Picker::from_query_stdio` 分支

- 位置：`src/app/mod.rs:~1839`
- 问题：结果被注释说明故意丢弃，属死代码。
- 建议：删除。

### 16. `apply_cover_fetch_result` — 封面字节双份内存

- 问题：`now_playing.cover` 与 queue slot 各存一份 `bytes`。
- 建议：仅 `now_playing` 持有，queue 里存 Arc。

### 17. `render_search_row` — 恒等式 + 每行 clone

- 位置：`src/ui/search.rs:174`
- 问题：`let right_style = if focused { row_style } else { row_style };` 恒等式；每行 clone `type_tag` / `right_label`。
- 建议：删恒等式，改借用。

### 18. `home_sidebar.anim_progress` — 伪动画

- 问题：只在 toggle 时被直接赋 0/1，从未随时间插值。
- 建议：要么实现缓动，要么删字段。

### 19. `sanitize_cache_key` — 切歌高频重建字符串

- 位置：`src/app/player.rs`
- 问题：每次 `cached_song_path` 调用都重新构建字符串。
- 建议：对 quality 层做 `OnceLock` / 常量映射。

### 20. `EqSource::next` — 每采样 10 次 atomic load

- 位置：`src/app/player.rs`
- 问题：每个音频采样执行 `load_db_x10()`（10 次 atomic load + 数组构造）。
- 建议：用「EQ 修订号」原子量，仅变化时重算。

---

## 建议实施顺序

1. **#1 切歌后台化** — 切歌卡顿，日常使用最痛的体验问题
2. **#6 签名缓存** — 全屏模式 CPU 占用
3. **#7/#8 渲染零拷贝** — 侧边栏/首页帧率
4. **#11/#12 流式写入优化** — 下载速度与磁盘压力
5. 其余作为清理项顺带处理
