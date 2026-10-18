pub mod db;
pub mod media;
pub mod reports;
pub mod store;

/// 所有錯誤都以可直接顯示給使用者的中文訊息傳回前端。
#[derive(Debug)]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;

pub fn bail<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error(msg.into()))
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error(db::message(&e))
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(format!("檔案存取失敗：{e}"))
    }
}

impl From<image::ImageError> for Error {
    fn from(e: image::ImageError) -> Self {
        Error(format!("圖片處理失敗：{e}"))
    }
}

impl From<rust_xlsxwriter::XlsxError> for Error {
    fn from(e: rust_xlsxwriter::XlsxError) -> Self {
        Error(format!("Excel 產生失敗：{e}"))
    }
}
