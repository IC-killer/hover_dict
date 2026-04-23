use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::error::Error;
use std::fs;

#[derive(Debug, Clone)]
pub struct TranslateResult {
    pub source_text: String,
    pub phonetic: Option<String>,
    pub translation: String,
    pub is_llm: bool,
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
        let conn = Connection::open_with_flags(&self.db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let word = text.trim().to_lowercase();

        let query = "SELECT phonetic, translation FROM dictionary WHERE word = ? LIMIT 1";
        let query_fallback = "SELECT phonetic, translation FROM stardict WHERE word = ? LIMIT 1";

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
                is_llm: false,
            }))
        } else {
            Ok(None)
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ModelItem {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ModelsConfig {
    pub active_model: String,
    pub api_endpoint: String,
    pub api_key_env_var: String,
    pub models: Vec<ModelItem>,
}

impl ModelsConfig {
    pub fn load() -> Self {
        let default_config = ModelsConfig {
            active_model: "tencent/Hunyuan-MT-7B".to_string(),
            api_endpoint: "https://api.siliconflow.cn/v1/chat/completions".to_string(),
            api_key_env_var: "SILICONFLOW_API_KEY".to_string(),
            models: vec![],
        };
        
        match fs::read_to_string("models.json") {
            Ok(content) => serde_json::from_str(&content).unwrap_or(default_config),
            Err(_) => default_config,
        }
    }

    pub fn save(&self) {
        if let Ok(content) = serde_json::to_string_pretty(self) {
            let _ = fs::write("models.json", content);
        }
    }
}

pub struct LlmTranslator;

impl LlmTranslator {
    pub fn translate(text: &str, config: &ModelsConfig) -> Result<Option<TranslateResult>, Box<dyn Error + Send + Sync>> {
        let api_key = std::env::var(&config.api_key_env_var).unwrap_or_default();
        if api_key.is_empty() {
            return Err("API key is missing. Please set environment variable.".into());
        }

        let client = reqwest::blocking::Client::new();
        
        let payload = json!({
            "model": config.active_model,
            "messages": [
                {
                    "role": "system",
                    "content": "你是一个专业的翻译助手。请将用户输入的文本翻译成中文。如果是中文，则翻译成英文。请只输出翻译结果，不要输出任何额外的解释或语气词。"
                },
                {
                    "role": "user",
                    "content": text
                }
            ]
        });

        let res = client.post(&config.api_endpoint)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&payload)
            .send()?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().unwrap_or_default();
            return Err(format!("API Request failed: {} - {}", status, body).into());
        }

        let res_json: serde_json::Value = res.json()?;
        
        if let Some(content) = res_json["choices"][0]["message"]["content"].as_str() {
            Ok(Some(TranslateResult {
                source_text: text.to_string(),
                phonetic: None,
                translation: content.trim().to_string(),
                is_llm: true,
            }))
        } else {
            Err("Failed to parse LLM response.".into())
        }
    }
}
