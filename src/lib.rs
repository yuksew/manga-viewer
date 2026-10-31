pub mod book;
pub mod cli;
pub mod cover;
pub mod decode;
pub mod drag;
pub mod imgcheck;
pub mod index;
pub mod natural;
pub mod neighbor;
pub mod scan;
pub mod server;
pub mod view;

/// 索引ルートの既定値（設計 A-2）。実データ path をコードに書く唯一の場所。
pub const DEFAULT_ROOT: &str = r"D:\books";
