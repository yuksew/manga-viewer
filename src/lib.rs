pub mod book;
pub mod cli;
pub mod index;
pub mod natural;
pub mod scan;

/// 索引ルートの既定値（設計 A-2）。実データ path をコードに書く唯一の場所。
pub const DEFAULT_ROOT: &str = r"D:\books";
