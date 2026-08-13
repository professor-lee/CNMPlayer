use super::Query;
use crate::error::Result;
/// 上传音频（声音）
/// 对应 Node.js module/voice_upload.js
///
/// 注意: 此端点涉及复杂的分块上传流程（multipart upload 到 NOS 对象存储）。
/// 由于 Rust SDK 是纯库，文件读取由调用者负责，需传入完整的音频二进制数据。
use crate::request::{ApiClient, ApiResponse, CryptoType};
use serde_json::json;

impl ApiClient {
    /// 上传音频文件
    ///
    /// query 参数:
    /// - `songName`: 歌曲名（可选，默认从文件名推断）
    /// - `autoPublish`: 是否自动发布 (1=是, 0=否)
    /// - `autoPublishText`: 自动发布文案
    /// - `description`: 描述
    /// - `voiceListId`: 声音列表 ID
    /// - `coverImgId`: 封面图 ID
    /// - `categoryId`: 分类 ID
    /// - `secondCategoryId`: 二级分类 ID
    /// - `composedSongs`: 关联歌曲 ID，逗号分隔
    /// - `privacy`: 是否私密 (1=是, 0=否)
    /// - `publishTime`: 发布时间
    /// - `orderNo`: 排序号
    ///
    /// `file_name`: 原始文件名（如 "song.mp3"）
    /// `file_data`: 音频文件的完整二进制数据
    /// `file_mimetype`: MIME 类型，如 "audio/mpeg"
    pub async fn voice_upload(
        &self,
        query: &Query,
        file_name: &str,
        file_data: Vec<u8>,
        file_mimetype: Option<&str>,
    ) -> Result<ApiResponse> {
        let mimetype = file_mimetype.unwrap_or("audio/mpeg");

        // 提取文件扩展名
        let ext = file_name.rsplit('.').next().unwrap_or("mp3");

        // 推断文件名
        let filename = if let Some(name) = query.get("songName") {
            name.to_string()
        } else {
            file_name
                .rsplit('.')
                .next_back()
                .unwrap_or(file_name)
                .replace(' ', "")
                .replace('.', "_")
        };

        // Step 1: 申请上传 token
        let token_data = json!({
            "bucket": "ymusic",
            "ext": ext,
            "filename": filename,
            "local": false,
            "nos_product": 0,
            "type": "other"
        });

        let token_res = self
            .request(
                "/api/nos/token/alloc",
                token_data,
                query.to_option(CryptoType::Weapi),
            )
            .await?;

        let result = &token_res.body["result"];
        let object_key_raw = result["objectKey"].as_str().unwrap_or_default();
        let object_key = object_key_raw.replace('/', "%2F");
        let token = result["token"].as_str().unwrap_or_default().to_string();
        let doc_id = result["docId"].clone();

        // Step 2: 初始化 multipart upload
        let init_url = format!("https://ymusic.nos-hz.163yun.com/{}?uploads", object_key);

        let init_res = self
            .client
            .post(&init_url)?
            .header("x-nos-token", &token)?
            .header("X-Nos-Meta-Content-Type", mimetype)?
            .send()
            .await
            .map_err(crate::error::NcmError::Http)?;

        let init_xml = init_res
            .text()
            .await
            .map_err(crate::error::NcmError::Http)?;

        // 简单解析 XML 获取 UploadId
        let upload_id = init_xml
            .split("<UploadId>")
            .nth(1)
            .and_then(|s| s.split("</UploadId>").next())
            .unwrap_or_default()
            .to_string();

        // Step 3: 分块上传（每块 10MB）
        let block_size = 10 * 1024 * 1024;
        let file_size = file_data.len();
        let mut offset = 0usize;
        let mut block_index = 1u32;
        let mut etags = Vec::new();

        while offset < file_size {
            let end = (offset + block_size).min(file_size);
            let chunk = file_data[offset..end].to_vec();

            let part_url = format!(
                "https://ymusic.nos-hz.163yun.com/{}?partNumber={}&uploadId={}",
                object_key, block_index, upload_id
            );

            let part_res = self
                .client
                .put(&part_url)?
                .header("x-nos-token", &token)?
                .header("Content-Type", mimetype)?
                .body(chunk)
                .send()
                .await
                .map_err(crate::error::NcmError::Http)?;

            if let Some(etag) = part_res.headers().get("etag") {
                etags.push(etag.to_str().unwrap_or_default().to_string());
            }

            offset = end;
            block_index += 1;
        }

        // Step 4: 完成 multipart upload
        let mut complete_xml = String::from("<CompleteMultipartUpload>");
        for (i, etag) in etags.iter().enumerate() {
            complete_xml.push_str(&format!(
                "<Part><PartNumber>{}</PartNumber><ETag>{}</ETag></Part>",
                i + 1,
                etag
            ));
        }
        complete_xml.push_str("</CompleteMultipartUpload>");

        let complete_url = format!(
            "https://ymusic.nos-hz.163yun.com/{}?uploadId={}",
            object_key, upload_id
        );

        self.client
            .post(&complete_url)?
            .header("Content-Type", "text/plain;charset=UTF-8")?
            .header("X-Nos-Meta-Content-Type", mimetype)?
            .header("x-nos-token", &token)?
            .body(complete_xml)
            .send()
            .await
            .map_err(crate::error::NcmError::Http)?;

        // Step 5: 生成 UUID 风格的 dupkey
        let dupkey = generate_dupkey();

        let auto_publish = query.get("autoPublish").map(|v| v == "1").unwrap_or(false);
        let privacy = query.get("privacy").map(|v| v == "1").unwrap_or(false);
        let composed_songs: Vec<String> = query
            .get("composedSongs")
            .map(|s| s.split(',').map(|x| x.to_string()).collect())
            .unwrap_or_default();

        let voice_data = json!([{
            "name": filename,
            "autoPublish": auto_publish,
            "autoPublishText": query.get_or("autoPublishText", ""),
            "description": query.get_or("description", ""),
            "voiceListId": query.get_or("voiceListId", ""),
            "coverImgId": query.get_or("coverImgId", ""),
            "dfsId": doc_id,
            "categoryId": query.get_or("categoryId", ""),
            "secondCategoryId": query.get_or("secondCategoryId", ""),
            "composedSongs": composed_songs,
            "privacy": privacy,
            "publishTime": query.get_or("publishTime", "0").parse::<i64>().unwrap_or(0),
            "orderNo": query.get_or("orderNo", "1").parse::<i64>().unwrap_or(1),
        }]);

        let voice_data_str = serde_json::to_string(&voice_data).unwrap_or_default();

        // Step 6: preCheck
        let _pre_check = self
            .request(
                "/api/voice/workbench/voice/batch/upload/preCheck",
                json!({
                    "dupkey": generate_dupkey(),
                    "voiceData": voice_data_str
                }),
                query.to_option(CryptoType::default()),
            )
            .await?;

        // Step 7: 实际上传
        let upload_result = self
            .request(
                "/api/voice/workbench/voice/batch/upload/v2",
                json!({
                    "dupkey": dupkey,
                    "voiceData": voice_data_str
                }),
                query.to_option(CryptoType::default()),
            )
            .await?;

        Ok(ApiResponse {
            status: 200,
            body: json!({
                "code": 200,
                "data": upload_result.body["data"]
            }),
            cookie: upload_result.cookie,
        })
    }
}

/// 生成 UUID v4 风格的 dupkey
fn generate_dupkey() -> String {
    use rand::RngExt;
    let hex_digits = b"0123456789abcdef";
    let mut rng = rand::rng();
    let mut s = [0u8; 36];

    for item in &mut s {
        *item = hex_digits[rng.random_range(0..16)];
    }
    s[14] = b'4';
    s[19] = hex_digits[((s[19] & 0x3) | 0x8) as usize];
    s[8] = b'-';
    s[13] = b'-';
    s[18] = b'-';
    s[23] = b'-';

    String::from_utf8_lossy(&s).to_string()
}
