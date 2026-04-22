use rusqlite::{Connection, OpenFlags};
use std::error::Error;

#[derive(Debug, Clone)]
pub struct TranslateResult {
    pub source_text: String,
    pub phonetic: Option<String>,
    pub translation: String,
}

pub struct LocalSqliteDict {
    db_path: String,
}

impl LocalSqliteDict {
    pub fn new(db_path: &str) -> Self {
        Self {
            db_path: db_path.to_string(),
        }
    }

    pub fn translate(
        &self,
        text: &str,
    ) -> Result<Option<TranslateResult>, Box<dyn Error + Send + Sync>> {
        // 【关键修复】使用 SQLITE_OPEN_READ_ONLY，如果文件不存在则直接报错，绝不自动创建空文件！
        let conn = Connection::open_with_flags(&self.db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let word = text.trim().to_lowercase();

        let query = "SELECT phonetic, translation FROM dictionary WHERE word = ? LIMIT 1";
        let query_fallback = "SELECT phonetic, translation FROM stardict WHERE word = ? LIMIT 1";

        // 兼容两种常见的表名
        let mut stmt = conn
            .prepare(query)
            .or_else(|_| conn.prepare(query_fallback))?;
        let mut rows = stmt.query([&word])?;

        if let Some(row) = rows.next()? {
            let phonetic: Option<String> = row.get(0).unwrap_or(None);
            let translation: String = row.get(1).unwrap_or_default();

            Ok(Some(TranslateResult {
                source_text: text.to_string(),
                phonetic: phonetic.filter(|s| !s.is_empty()),
                translation: translation.replace("\\n", "\n").replace("\\r", ""),
            }))
        } else {
            Ok(None)
        }
    }
}
