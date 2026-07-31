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

/// Get all chat history (newest first — matches display order)
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


/// Escape a string for safe embedding inside a JS single-quoted string
fn escape_js(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Generate HTML for chat history.
/// `avatar_data_uri` is a base64 data URI (or empty string if no avatar set).
fn generate_html(messages: &[ChatMessage], avatar_data_uri: &str) -> String {
    // Build JSON array of messages for client-side pagination
    let json_messages: String = messages.iter()
        .map(|m| {
            format!(
                "{{ts:'{}',role:'{}',content:'{}'}}",
                escape_js(&m.timestamp),
                escape_js(&m.role),
                escape_js(&m.content)
            )
        })
        .collect::<Vec<_>>()
        .join(",");

    let avatar_html = if avatar_data_uri.is_empty() {
        "🤖".to_string()
    } else {
        format!("<img src=\"{}\" alt=\"avatar\">", avatar_data_uri)
    };

    // Chrono's RFC3339 → "YYYY-MM-DD" extraction in JS via Date parsing
    // Tauri KV store path for avatar fallback note
    format!(r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Pocket Agent - Chat History</title>
    <style>
        * {{ margin: 0; padding: 0; box-sizing: border-box; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: linear-gradient(135deg, #0a0a16 0%, #1a1a2e 100%);
            color: #e8e8f0;
            padding: 20px;
            min-height: 100vh;
        }}
        .container {{ max-width: 900px; margin: 0 auto; }}

        /* ── Header ── */
        .header {{
            display: flex; justify-content: space-between; align-items: center;
            margin-bottom: 24px; padding-bottom: 16px;
            border-bottom: 1px solid rgba(160, 168, 255, 0.2);
            position: sticky; top: 0; z-index: 100;
            background: linear-gradient(135deg, #0a0a16 0%, #1a1a2e 100%);
            padding-top: 4px;
        }}
        .header h1 {{ font-size: 24px; font-weight: 600; }}
        .search-box {{ display: flex; gap: 10px; }}
        .search-box input {{
            padding: 8px 16px; border-radius: 8px;
            border: 1px solid rgba(160, 168, 255, 0.3);
            background: rgba(255, 255, 255, 0.05);
            color: #e8e8f0; font-size: 14px; width: 250px;
        }}
        .search-box input::placeholder {{ color: rgba(232, 232, 240, 0.4); }}
        .search-box input:focus {{
            outline: none; border-color: rgba(160, 168, 255, 0.6);
            background: rgba(255, 255, 255, 0.08);
        }}

        .month-picker {{
            padding: 7px 10px; border-radius: 8px;
            border: 1px solid rgba(160, 168, 255, 0.3);
            background: rgba(255, 255, 255, 0.05);
            color: #e8e8f0; font-size: 13px;
            cursor: pointer; color-scheme: dark;
        }}
        .month-picker:focus {{
            outline: none; border-color: rgba(160, 168, 255, 0.6);
        }}
        .month-picker::-webkit-calendar-picker-indicator {{
            filter: invert(0.8); cursor: pointer;
        }}

        /* Calendar day grid */
        .calendar-grid {{
            display: grid;
            grid-template-columns: repeat(7, 1fr);
            gap: 4px;
            margin: 8px 0 4px;
        }}
        .calendar-wrap {{
            overflow: hidden;
            transition: max-height 0.2s ease;
        }}
        .calendar-wrap.collapsed {{
            max-height: 0;
        }}
        .calendar-wrap.expanded {{
            max-height: 300px;
        }}
        .cal-toggle {{
            display: inline-flex; align-items: center; gap: 4px;
            padding: 4px 12px; border-radius: 6px;
            border: 1px solid rgba(160, 168, 255, 0.2);
            background: rgba(160, 168, 255, 0.06);
            color: rgba(160, 168, 255, 0.7);
            font-size: 12px; cursor: pointer;
            transition: background 0.12s;
        }}
        .cal-toggle:hover {{
            background: rgba(160, 168, 255, 0.14);
        }}
        .cal-header {{
            text-align: center; font-size: 11px; font-weight: 600;
            color: rgba(160, 168, 255, 0.5);
            padding: 4px 0;
        }}
        .cal-day {{
            height: 28px;
            display: flex; align-items: center; justify-content: center;
            border-radius: 8px; font-size: 13px;
            cursor: default; user-select: none;
            background: rgba(255, 255, 255, 0.03);
            color: rgba(232, 232, 240, 0.2);
            border: 1px solid transparent;
            transition: all 0.12s;
        }}
        .cal-day.has-msg {{
            background: rgba(124, 158, 255, 0.15);
            color: rgba(160, 168, 255, 0.9);
            border-color: rgba(124, 158, 255, 0.3);
            cursor: pointer; font-weight: 600;
        }}
        .cal-day.has-msg:hover {{
            background: rgba(124, 158, 255, 0.3);
            border-color: rgba(124, 158, 255, 0.5);
        }}
        .cal-day.selected {{
            background: rgba(124, 158, 255, 0.4);
            border-color: rgba(124, 158, 255, 0.8);
        }}
        .cal-day.empty {{
            background: transparent; border: none;
        }}

        /* ── Messages ── */
        .messages {{ display: flex; flex-direction: column; gap: 14px; }}

        /* ── Date separator ── */
        .date-separator {{
            display: flex; align-items: center; gap: 12px;
            margin: 20px 0 8px; padding: 0 20px;
        }}
        .date-separator::before, .date-separator::after {{
            content: ''; flex: 1; height: 1px;
            background: rgba(160, 168, 255, 0.15);
        }}
        .date-separator span {{
            font-size: 13px; font-weight: 500;
            color: rgba(160, 168, 255, 0.7);
            white-space: nowrap;
        }}

        /* ── Message row with avatar ── */
        .message-row {{
            display: flex; gap: 10px; align-items: flex-start;
            animation: fadeIn 0.2s ease-out;
        }}
        .message-row.user {{ flex-direction: row-reverse; }}

        .avatar {{
            width: 36px; height: 36px; border-radius: 50%;
            flex-shrink: 0; overflow: hidden;
            display: flex; align-items: center; justify-content: center;
            background: rgba(160, 168, 255, 0.12);
            border: 1px solid rgba(160, 168, 255, 0.2);
            font-size: 18px;
        }}
        .avatar img {{ width: 100%; height: 100%; object-fit: cover; }}

        .message {{
            flex: 1; max-width: 72%;
            padding: 12px 16px; border-radius: 14px;
            border: 1px solid rgba(160, 168, 255, 0.2);
            background: rgba(10, 10, 22, 0.6);
        }}
        .message.user {{
            background: rgba(124, 158, 255, 0.1);
            border-color: rgba(124, 158, 255, 0.3);
            margin-right: 80px;
        }}
        .message.assistant {{
            background: rgba(160, 168, 255, 0.08);
            border-color: rgba(160, 168, 255, 0.2);
            margin-left: 80px;
        }}
        /* Override: with avatar layout, margins create the offset */
        .message-row.assistant .message {{ margin-left: 0; margin-right: 80px; }}
        .message-row.user .message {{ margin-right: 0; margin-left: 80px; }}

        .message-header {{
            display: flex; justify-content: space-between; align-items: center;
            margin-bottom: 6px; font-size: 11px;
        }}
        .message-role {{ font-weight: 600; color: rgba(160, 168, 255, 0.9); text-transform: uppercase; }}
        .message-time {{ color: rgba(232, 232, 240, 0.45); }}

        .message-content {{
            font-size: 14px; line-height: 1.6;
            color: rgba(232, 232, 240, 0.92);
            word-break: break-word; white-space: pre-wrap;
        }}
        .message-content a {{ color: rgba(124, 158, 255, 0.9); text-decoration: underline; }}
        .message-content a:hover {{ color: rgba(160, 168, 255, 1); }}

        @keyframes fadeIn {{
            from {{ opacity: 0; transform: translateY(8px); }}
            to   {{ opacity: 1; transform: translateY(0); }}
        }}

        /* ── Load more ── */
        .empty {{ text-align: center; padding: 60px 20px; color: rgba(232, 232, 240, 0.4); }}
        .empty-icon {{ font-size: 48px; margin-bottom: 16px; }}

        .stats {{
            margin-top: 24px; padding: 12px 16px; border-radius: 8px;
            background: rgba(160, 168, 255, 0.06);
            border: 1px solid rgba(160, 168, 255, 0.15);
            font-size: 12px; color: rgba(232, 232, 240, 0.6);
            text-align: center;
        }}
    </style>
</head>
<body>
    <div class="container">
        <div class="header">
            <h1>Chat History</h1>
            <div class="search-box">
                <input type="month" class="month-picker" id="monthPicker">
                <input type="text" id="searchInput" placeholder="Search messages...">
            </div>
        </div>
        <span class="cal-toggle" id="calToggle">Date Picker ▲</span>
        <div class="calendar-wrap expanded" id="calWrap">
            <div class="calendar-grid" id="calendarGrid"></div>
            <hr style="border:none;border-top:1px solid rgba(160,168,255,0.15);margin:4px 0 16px;">
        </div>
        <div class="messages" id="messagesContainer"></div>
        <div class="stats" id="statsBar"></div>
    </div>
    <script>
    (function() {{
        const ALL = [{json_messages}];
        const AVATAR_HTML = `{avatar_html}`;

        function parseTs(ts) {{
            const d = new Date(ts);
            const dateKey = d.getFullYear() + '-' +
                String(d.getMonth()+1).padStart(2,'0') + '-' +
                String(d.getDate()).padStart(2,'0');
            const timeStr = String(d.getHours()).padStart(2,'0') + ':' +
                String(d.getMinutes()).padStart(2,'0');
            const weekdays = ['Sun','Mon','Tue','Wed','Thu','Fri','Sat'];
            const months = ['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'];
            let label = weekdays[d.getDay()] + ', ' + months[d.getMonth()] + ' ' + d.getDate();
            const today = new Date();
            const tk = today.getFullYear() + '-' + String(today.getMonth()+1).padStart(2,'0') + '-' + String(today.getDate()).padStart(2,'0');
            const ys = new Date(today); ys.setDate(ys.getDate()-1);
            const yk = ys.getFullYear() + '-' + String(ys.getMonth()+1).padStart(2,'0') + '-' + String(ys.getDate()).padStart(2,'0');
            if (dateKey === tk) label = 'Today';
            else if (dateKey === yk) label = 'Yesterday';
            return {{ dateKey, timeStr, label, day: d.getDate(), monthKey: d.getFullYear()+'-'+String(d.getMonth()+1).padStart(2,'0') }};
        }}

        function sanitizeContent(html) {{
            const div = document.createElement('div');
            div.innerHTML = html;
            div.querySelectorAll('script').forEach(el => el.remove());
            div.querySelectorAll('*').forEach(el => {{
                Array.from(el.attributes).forEach(a => {{
                    if (a.name.startsWith('on')) el.removeAttribute(a.name);
                }});
            }});
            return div.innerHTML;
        }}

        // Build month -> dateKey -> count map
        const msgByDate = {{}};
        const monthSet = new Set();
        ALL.forEach(m => {{
            const p = parseTs(m.ts);
            if (!msgByDate[p.dateKey]) msgByDate[p.dateKey] = 0;
            msgByDate[p.dateKey]++;
            monthSet.add(p.monthKey);
        }});
        const allMonths = [...monthSet].sort().reverse();

        // State
        let selectedMonth = '';
        let selectedDate = '';

        // ── Calendar grid ──
        const calGrid = document.getElementById('calendarGrid');
        const weekdaysShort = ['Mon','Tue','Wed','Thu','Fri','Sat','Sun'];

        function renderCalendar() {{
            calGrid.innerHTML = '';
            // Weekday headers
            weekdaysShort.forEach(w => {{
                const h = document.createElement('div');
                h.className = 'cal-header';
                h.textContent = w;
                calGrid.appendChild(h);
            }});
            if (!selectedMonth) return;

            const [yr, mo] = selectedMonth.split('-').map(Number);
            const firstDay = new Date(yr, mo - 1, 1);
            const daysInMonth = new Date(yr, mo, 0).getDate();
            // JS getDay: 0=Sun..6=Sat. We want Mon=0..Sun=6
            let startCol = firstDay.getDay() - 1;
            if (startCol < 0) startCol = 6;

            // Empty cells before day 1
            for (let i = 0; i < startCol; i++) {{
                const e = document.createElement('div');
                e.className = 'cal-day empty';
                calGrid.appendChild(e);
            }}

            // Day cells
            for (let day = 1; day <= daysInMonth; day++) {{
                const dateKey = selectedMonth + '-' + String(day).padStart(2,'0');
                const cell = document.createElement('div');
                cell.className = 'cal-day';
                cell.textContent = day;
                if (msgByDate[dateKey]) {{
                    cell.classList.add('has-msg');
                    cell.title = msgByDate[dateKey] + ' messages';
                    cell.dataset.date = dateKey;
                    if (dateKey === selectedDate) cell.classList.add('selected');
                    cell.addEventListener('click', () => {{
                        selectedDate = dateKey;
                        document.querySelectorAll('.cal-day.selected').forEach(c => c.classList.remove('selected'));
                        cell.classList.add('selected');
                        renderMessages();
                    }});
                }}
                calGrid.appendChild(cell);
            }}
        }}

        // ── Render messages for selectedDate ──
        function renderMessages() {{
            const container = document.getElementById('messagesContainer');
            container.innerHTML = '';

            if (!selectedDate) {{
                container.innerHTML = '<div class="empty"><p>Select a highlighted day above</p></div>';
                updateStats(0);
                return;
            }}

            const dayMsgs = ALL.filter(m => {{
                const p = parseTs(m.ts);
                if (p.dateKey !== selectedDate) return false;
                const q = document.getElementById('searchInput').value.toLowerCase().trim();
                if (q && !m.content.toLowerCase().includes(q)) return false;
                return true;
            }}).reverse(); // oldest first for display

            if (dayMsgs.length === 0) {{
                const p = parseTs(selectedDate + 'T00:00:00');
                container.innerHTML = '<div class="empty"><p>No messages on ' + p.label + '</p></div>';
                updateStats(0);
                return;
            }}

            const dateInfo = parseTs(dayMsgs[0].ts);
            const sep = document.createElement('div');
            sep.className = 'date-separator';
            sep.innerHTML = '<span>' + dateInfo.label + '</span>';
            container.appendChild(sep);

            dayMsgs.forEach(msg => {{
                const p = parseTs(msg.ts);
                const isUser = msg.role === 'user';
                const row = document.createElement('div');
                row.className = 'message-row ' + (isUser ? 'user' : 'assistant');
                const avatarHtml = isUser ? '' : '<div class="avatar">' + AVATAR_HTML + '</div>';
                row.innerHTML = avatarHtml +
                    '<div class="message ' + (isUser ? 'user' : 'assistant') + '">' +
                        '<div class="message-header">' +
                            '<span class="message-role">' + (isUser ? 'You' : 'PA') + '</span>' +
                            '<span class="message-time">' + p.timeStr + '</span>' +
                        '</div>' +
                        '<div class="message-content">' + sanitizeContent(msg.content) + '</div>' +
                    '</div>';
                container.appendChild(row);
            }});
            updateStats(dayMsgs.length);
        }}

        function updateStats(count) {{
            document.getElementById('statsBar').textContent =
                selectedDate ? (selectedDate + ': ' + count + ' messages') : '';
        }}

        // ── Month picker ──
        const monthPicker = document.getElementById('monthPicker');
        if (allMonths.length > 0) {{
            selectedMonth = allMonths[0]; // default to most recent month with messages
            monthPicker.value = selectedMonth;
            // Auto-select the most recent day with messages in this month
            for (const m of ALL) {{
                const p = parseTs(m.ts);
                if (p.monthKey === selectedMonth) {{
                    selectedDate = p.dateKey;
                    break;
                }}
            }}
        }}
        monthPicker.addEventListener('change', (e) => {{
            const v = e.target.value;
            if (v) {{
                selectedMonth = v;
                // Find the latest day with messages in this month
                selectedDate = '';
                for (const m of ALL) {{
                    const p = parseTs(m.ts);
                    if (p.monthKey === v) {{
                        selectedDate = p.dateKey;
                        break;
                    }}
                }}
                renderCalendar();
                renderMessages();
            }}
        }});

        // ── Search ──
        let searchTimer = null;
        document.getElementById('searchInput').addEventListener('input', () => {{
            clearTimeout(searchTimer);
            searchTimer = setTimeout(renderMessages, 200);
        }});

        // Calendar collapse toggle
        const calToggle = document.getElementById('calToggle');
        const calWrap = document.getElementById('calWrap');
        calToggle.addEventListener('click', () => {{
            if (calWrap.classList.contains('expanded')) {{
                calWrap.classList.remove('expanded');
                calWrap.classList.add('collapsed');
                calToggle.textContent = 'Date Picker ▼';
            }} else {{
                calWrap.classList.remove('collapsed');
                calWrap.classList.add('expanded');
                calToggle.textContent = 'Date Picker ▲';
            }}
        }});

        // Initial render
        renderCalendar();
        renderMessages();
    }})();
    </script>
</body>
</html>"#)
}

/// Generate and open chat history HTML
#[tauri::command]
pub async fn open_chat_history(_app: tauri::AppHandle) -> Result<(), String> {
    let messages = get_all_messages()?;

    // Read the current avatar from settings.db, the settings source of truth.
    let avatar_data_uri = super::settings_repository::get_asset("avatar_image")?
        .unwrap_or_default();

    let html = generate_html(&messages, &avatar_data_uri);
    
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
