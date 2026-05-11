use rusqlite::{Connection, params};
use std::path::PathBuf;
use chrono::Local;

/// Get the chat history database path
fn get_db_path() -> PathBuf {
    let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push(".pocket-agent");
    std::fs::create_dir_all(&path).ok();
    path.push("chat_history.db");
    path
}

/// Initialize the database
pub fn init_db() -> Result<(), String> {
    let db_path = get_db_path();
    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
    
    conn.execute(
        "CREATE TABLE IF NOT EXISTS chat_history (
            id INTEGER PRIMARY KEY,
            timestamp TEXT NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            session_id TEXT
        )",
        [],
    ).map_err(|e| e.to_string())?;
    
    Ok(())
}

/// Save a chat message to history
pub fn save_message(role: &str, content: &str) -> Result<(), String> {
    let db_path = get_db_path();
    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
    
    let timestamp = Local::now().to_rfc3339();
    let session_id = format!("{}", Local::now().format("%Y-%m-%d"));
    
    conn.execute(
        "INSERT INTO chat_history (timestamp, role, content, session_id) VALUES (?1, ?2, ?3, ?4)",
        params![&timestamp, role, content, &session_id],
    ).map_err(|e| e.to_string())?;
    
    Ok(())
}

/// Tauri command to save a message
#[tauri::command]
pub fn save_chat_message(role: String, content: String) -> Result<(), String> {
    save_message(&role, &content)
}

#[derive(serde::Serialize)]
pub struct ChatMessage {
    pub timestamp: String,
    pub role: String,
    pub content: String,
}

/// Get all chat history
pub fn get_all_messages() -> Result<Vec<ChatMessage>, String> {
    let db_path = get_db_path();
    let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
    
    let mut stmt = conn.prepare(
        "SELECT timestamp, role, content FROM chat_history ORDER BY id DESC"
    ).map_err(|e| e.to_string())?;
    
    let messages = stmt.query_map([], |row| {
        Ok(ChatMessage {
            timestamp: row.get(0)?,
            role: row.get(1)?,
            content: row.get(2)?,
        })
    }).map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    
    Ok(messages)
}

/// Generate HTML for chat history
fn generate_html(messages: &[ChatMessage]) -> String {
    let mut html = String::from(r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Pocket Agent - Chat History</title>
    <style>
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }
        
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: linear-gradient(135deg, #0a0a16 0%, #1a1a2e 100%);
            color: #e8e8f0;
            padding: 20px;
            min-height: 100vh;
        }
        
        .container {
            max-width: 900px;
            margin: 0 auto;
        }
        
        .header {
            display: flex;
            justify-content: space-between;
            align-items: center;
            margin-bottom: 30px;
            padding-bottom: 20px;
            border-bottom: 1px solid rgba(160, 168, 255, 0.2);
        }
        
        .header h1 {
            font-size: 28px;
            font-weight: 600;
        }
        
        .search-box {
            display: flex;
            gap: 10px;
        }
        
        .search-box input {
            padding: 8px 16px;
            border-radius: 8px;
            border: 1px solid rgba(160, 168, 255, 0.3);
            background: rgba(255, 255, 255, 0.05);
            color: #e8e8f0;
            font-size: 14px;
            width: 250px;
        }
        
        .search-box input::placeholder {
            color: rgba(232, 232, 240, 0.4);
        }
        
        .search-box input:focus {
            outline: none;
            border-color: rgba(160, 168, 255, 0.6);
            background: rgba(255, 255, 255, 0.08);
        }
        
        .messages {
            display: flex;
            flex-direction: column;
            gap: 16px;
        }
        
        .message {
            padding: 16px;
            border-radius: 12px;
            border: 1px solid rgba(160, 168, 255, 0.2);
            background: rgba(10, 10, 22, 0.6);
            backdrop-filter: blur(10px);
            animation: fadeIn 0.3s ease-out;
        }
        
        @keyframes fadeIn {
            from {
                opacity: 0;
                transform: translateY(10px);
            }
            to {
                opacity: 1;
                transform: translateY(0);
            }
        }
        
        .message.user {
            background: rgba(124, 158, 255, 0.1);
            border-color: rgba(124, 158, 255, 0.3);
            margin-left: 40px;
        }
        
        .message.assistant {
            background: rgba(160, 168, 255, 0.08);
            border-color: rgba(160, 168, 255, 0.2);
            margin-right: 40px;
        }
        
        .message-header {
            display: flex;
            justify-content: space-between;
            align-items: center;
            margin-bottom: 8px;
            font-size: 12px;
        }
        
        .message-role {
            font-weight: 600;
            color: rgba(160, 168, 255, 0.9);
            text-transform: uppercase;
        }
        
        .message-time {
            color: rgba(232, 232, 240, 0.5);
            font-size: 11px;
        }
        
        .message-content {
            font-size: 14px;
            line-height: 1.6;
            color: rgba(232, 232, 240, 0.92);
            word-break: break-word;
            white-space: pre-wrap;
        }
        
        .message-content a {
            color: rgba(124, 158, 255, 0.9);
            text-decoration: underline;
        }
        
        .message-content a:hover {
            color: rgba(160, 168, 255, 1);
        }
        
        .empty {
            text-align: center;
            padding: 60px 20px;
            color: rgba(232, 232, 240, 0.4);
        }
        
        .empty-icon {
            font-size: 48px;
            margin-bottom: 16px;
        }
        
        .stats {
            margin-top: 30px;
            padding: 16px;
            border-radius: 8px;
            background: rgba(160, 168, 255, 0.08);
            border: 1px solid rgba(160, 168, 255, 0.2);
            font-size: 13px;
            color: rgba(232, 232, 240, 0.7);
        }
    </style>
</head>
<body>
    <div class="container">
        <div class="header">
            <h1>💬 Chat History</h1>
            <div class="search-box">
                <input type="text" id="searchInput" placeholder="Search messages...">
            </div>
        </div>
        
        <div class="messages" id="messagesContainer">
"#);
    
    if messages.is_empty() {
        html.push_str(r#"            <div class="empty">
                <div class="empty-icon">📭</div>
                <p>No chat history yet</p>
            </div>
"#);
    } else {
        for msg in messages {
            let role_class = if msg.role == "user" { "user" } else { "assistant" };
            let role_display = if msg.role == "user" { "You" } else { "Assistant" };
            
            html.push_str(&format!(
                r#"            <div class="message {}" data-content="{}">
                <div class="message-header">
                    <span class="message-role">{}</span>
                    <span class="message-time">{}</span>
                </div>
                <div class="message-content">{}</div>
            </div>
"#,
                role_class,
                escape_html(&msg.content),
                role_display,
                msg.timestamp,
                escape_html(&msg.content)
            ));
        }
    }
    
    html.push_str(r#"        </div>
        
        <div class="stats">
            <strong>Total messages:</strong> <span id="totalCount">0</span> | 
            <strong>User:</strong> <span id="userCount">0</span> | 
            <strong>Assistant:</strong> <span id="assistantCount">0</span>
        </div>
    </div>
    
    <script>
        const searchInput = document.getElementById('searchInput');
        const messagesContainer = document.getElementById('messagesContainer');
        const messages = messagesContainer.querySelectorAll('.message');
        
        // Update stats
        const totalCount = messages.length;
        const userCount = messagesContainer.querySelectorAll('.message.user').length;
        const assistantCount = messagesContainer.querySelectorAll('.message.assistant').length;
        
        document.getElementById('totalCount').textContent = totalCount;
        document.getElementById('userCount').textContent = userCount;
        document.getElementById('assistantCount').textContent = assistantCount;
        
        // Search functionality
        searchInput.addEventListener('input', (e) => {
            const query = e.target.value.toLowerCase();
            messages.forEach(msg => {
                const content = msg.getAttribute('data-content').toLowerCase();
                msg.style.display = content.includes(query) ? 'block' : 'none';
            });
        });
    </script>
</body>
</html>"#);
    
    html
}

/// Escape HTML special characters
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Generate and open chat history HTML
#[tauri::command]
pub async fn open_chat_history(_app: tauri::AppHandle) -> Result<(), String> {
    let messages = get_all_messages()?;
    let html = generate_html(&messages);
    
    let html_path = {
        let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        path.push(".pocket-agent");
        path.push("chat-history.html");
        path
    };
    
    std::fs::write(&html_path, html).map_err(|e| e.to_string())?;
    
    // Open in default browser
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&html_path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(&["/C", "start", html_path.to_str().unwrap_or("")])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&html_path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    
    Ok(())
}
