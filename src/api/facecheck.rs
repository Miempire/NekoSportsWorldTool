//! 人脸抽检上传（按官方 `PartnerServer.faceContract` 实现）。
//!
//! ```java
//! @ApiCrypto(decryptResponse = false, encryptBody = false)
//! @POST("api/v70100/run/facecheck/save")
//! @Multipart
//! Object faceContract(@Part MultipartBody.Part part, @Query("status") int i, ...);
//! ```
//!
//! 关键差异（与早期文档/猜测不同）：
//! - baseUrl 是 **`https://partner.iydsj.com`**，不是 run 域；
//! - Multipart **只有一个 part**，名字 `file`；没有 type/distance/... 这些字段；
//! - body 不加密、响应不解密（`@ApiCrypto` 两个开关都是 false），所以响应是明文 JSON；
//! - 抽检记录明细（type/distance/verifyType/checkStatus/failureReason）走的是另一个
//!   接口 `POST api/v70240/run/pv`（`CommonServer.getRunPv`），body
//!   `{unid, uuid, runRecordDetails:[...]}`，见 [`upload_record_details`]。

use super::client::ApiClient;
use crate::crypto::header::{UA_ANDROID, UA_IOS};
use serde_json::{json, Value};

pub const PARTNER_HOST: &str = "https://partner.iydsj.com";
pub const FACECHECK_PATH: &str = "/api/v70100/run/facecheck/save";
/// 抽检记录明细（`getRunPv`）。
pub const RUN_PV_PATH: &str = "/api/v70240/run/pv";

fn ua_for(client: &ApiClient) -> &'static str {
    if client.identity.platform == "android" {
        UA_ANDROID
    } else {
        UA_IOS
    }
}

/// 上传一张抽检照片。`status` 是 query 参数，官方从抽检回执里取。
pub fn upload(
    client: &mut ApiClient,
    status: i64,
    photo: &[u8],
    photo_name: &str,
) -> Result<Value, String> {
    if photo.is_empty() {
        return Err("照片为空".into());
    }
    let boundary = format!("----NekoBoundary{}", crate::crypto::envelope::now_ms());
    let mut body = Vec::with_capacity(photo.len() + 256);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{photo_name}\"\r\nContent-Type: image/jpeg\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(photo);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let url = format!("{PARTNER_HOST}{FACECHECK_PATH}?status={status}");
    let content_type = format!("multipart/form-data; boundary={boundary}");
    let ua = ua_for(client);
    let out = client.raw_request("POST", &url, body, &content_type, ua, &[])?;
    match out.decrypted {
        Some(decrypted) => Ok(decrypted.business),
        None => Err(format!(
            "facecheck 响应解析失败（HTTP {}）{}",
            out.http_status, out.raw_head
        )),
    }
}

/// 上报抽检记录明细（`api/v70240/run/pv`）。
///
/// `details` 里每项形如
/// `{"type":1,"distance":123,"verifyType":1,"checkStatus":true,"failureReason":0}`。
pub fn upload_record_details(
    client: &mut ApiClient,
    unid: i64,
    uuid: &str,
    details: Value,
) -> Result<Value, String> {
    let body = json!({
        "unid": unid,
        "uuid": uuid,
        "runRecordDetails": details,
    })
    .to_string();
    client.call("POST", RUN_PV_PATH, &body, &[])
}
