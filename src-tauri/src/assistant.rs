//! In-app AI assistant. Runs Claude Code headlessly (`claude -p`) in a small
//! workspace that loads the Music Maker MCP server and composing skill, and
//! streams its replies to the UI as "assistant-event" events. Uses the user's
//! existing Claude Code login; no API key is stored here.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

const CLAUDE_MD: &str = include_str!("../../tools/assistant/CLAUDE.md");
const SKILL_MD: &str = include_str!("../../.claude/skills/compose-music/SKILL.md");
const MCP_JS: &str = include_str!("../../tools/mcp/music-maker-mcp.mjs");

#[derive(Default)]
struct State {
    session: Option<String>,
    child: Option<Arc<Mutex<Child>>>,
    workspace: Option<PathBuf>,
    data_dir: Option<PathBuf>,
}

static STATE: Mutex<State> = Mutex::new(State { session: None, child: None, workspace: None, data_dir: None });

/// Writes the assistant workspace (CLAUDE.md, skill, MCP server + config) into the app data folder.
pub fn setup(data_dir: &Path) -> Result<(), String> {
    let ws = data_dir.join("assistant");
    let skill_dir = ws.join(".claude").join("skills").join("compose-music");
    std::fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;
    let w = |p: PathBuf, s: &str| std::fs::write(&p, s).map_err(|e| format!("{}: {e}", p.display()));
    w(ws.join("CLAUDE.md"), CLAUDE_MD)?;
    w(skill_dir.join("SKILL.md"), SKILL_MD)?;
    let mcp = ws.join("music-maker-mcp.mjs");
    w(mcp.clone(), MCP_JS)?;
    let cfg = json!({
        "mcpServers": {
            "music-maker": {
                "command": "node",
                "args": [mcp.to_string_lossy()],
                "env": { "MUSIC_MAKER_CONTROL": data_dir.join("control.json").to_string_lossy() }
            }
        }
    });
    w(ws.join("mcp.json"), &serde_json::to_string_pretty(&cfg).unwrap())?;
    let mut st = STATE.lock().unwrap();
    st.workspace = Some(ws);
    st.data_dir = Some(data_dir.to_path_buf());
    Ok(())
}

fn hide(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

pub fn find_claude() -> Option<PathBuf> {
    let mut candidates = vec![PathBuf::from("claude")];
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = PathBuf::from(home);
        candidates.push(home.join(".local").join("bin").join("claude.exe"));
        candidates.push(home.join(".local").join("bin").join("claude"));
        candidates.push(home.join("AppData").join("Roaming").join("npm").join("claude.cmd"));
    }
    candidates.into_iter().find(|c| {
        let mut cmd = Command::new(c);
        cmd.arg("--version").stdout(Stdio::null()).stderr(Stdio::null());
        hide(&mut cmd);
        cmd.status().map(|s| s.success()).unwrap_or(false)
    })
}

pub fn status() -> Value {
    let st = STATE.lock().unwrap();
    json!({
        "available": find_claude().is_some(),
        "busy": st.child.is_some(),
        "hasSession": st.session.is_some(),
    })
}

pub fn reset() {
    STATE.lock().unwrap().session = None;
}

pub fn cancel() {
    if let Some(c) = STATE.lock().unwrap().child.take() {
        let _ = c.lock().unwrap().kill();
    }
}

fn short_tool_name(n: &str) -> String {
    n.strip_prefix("mcp__music-maker__").unwrap_or(n).replace('_', " ")
}

/// Starts a request. Replies arrive as "assistant-event" events:
/// {kind: "text"|"tool"|"done"|"error", text}
pub fn ask(app: AppHandle, prompt: String, context: Value) -> Result<(), String> {
    let claude = find_claude().ok_or(
        "Claude Code wasn't found. Install it (https://claude.com/claude-code) and sign in once by running `claude` in a terminal.",
    )?;
    let (ws, session) = {
        let st = STATE.lock().unwrap();
        if st.child.is_some() {
            return Err("The assistant is still working on the last request.".into());
        }
        (st.workspace.clone().ok_or("Assistant workspace not set up")?, st.session.clone())
    };
    let mut args: Vec<String> = vec![
        "-p".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--mcp-config".into(),
        ws.join("mcp.json").to_string_lossy().into_owned(),
        "--strict-mcp-config".into(),
        "--allowedTools".into(),
        "mcp__music-maker".into(),
        "Skill".into(),
        "--disallowedTools".into(),
        "Bash".into(),
        "Edit".into(),
        "Write".into(),
        "NotebookEdit".into(),
    ];
    if let Some(s) = &session {
        args.push("--resume".into());
        args.push(s.clone());
    }
    let full_prompt = format!(
        "<music-maker-context>\n{}\n</music-maker-context>\n\n{}",
        serde_json::to_string_pretty(&context).unwrap_or_default(),
        prompt
    );
    let mut cmd = Command::new(&claude);
    cmd.args(&args).current_dir(&ws).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    hide(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("Couldn't start Claude Code: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut stderr = child.stderr.take().ok_or("no stderr")?;
    std::thread::spawn(move || {
        let _ = stdin.write_all(full_prompt.as_bytes());
    });
    let child = Arc::new(Mutex::new(child));
    STATE.lock().unwrap().child = Some(child.clone());

    let err_buf = Arc::new(Mutex::new(String::new()));
    let eb = err_buf.clone();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        *eb.lock().unwrap() = s;
    });

    std::thread::spawn(move || {
        let emit = |kind: &str, text: String| {
            let _ = app.emit("assistant-event", json!({ "kind": kind, "text": text }));
        };
        let mut got_result = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if let Some(sid) = v.get("session_id").and_then(|s| s.as_str()) {
                STATE.lock().unwrap().session = Some(sid.to_string());
            }
            match v.get("type").and_then(|t| t.as_str()) {
                Some("assistant") => {
                    for block in v["message"]["content"].as_array().cloned().unwrap_or_default() {
                        match block.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                let t = block["text"].as_str().unwrap_or("").trim().to_string();
                                if !t.is_empty() {
                                    emit("text", t);
                                }
                            }
                            Some("tool_use") => emit("tool", short_tool_name(block["name"].as_str().unwrap_or("tool"))),
                            _ => {}
                        }
                    }
                }
                Some("result") => {
                    got_result = true;
                    if v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false) {
                        emit("error", v.get("result").and_then(|r| r.as_str()).unwrap_or("The assistant hit an error.").to_string());
                    } else {
                        let cost = v.get("total_cost_usd").and_then(|c| c.as_f64());
                        emit("done", cost.map(|c| format!("${c:.3}")).unwrap_or_default());
                    }
                }
                _ => {}
            }
        }
        let status = child.lock().unwrap().wait();
        STATE.lock().unwrap().child = None;
        if !got_result {
            let err = err_buf.lock().unwrap().clone();
            let killed = status.as_ref().map(|s| !s.success()).unwrap_or(true) && err.trim().is_empty();
            if killed {
                emit("error", "Stopped.".into());
            } else {
                let tail: Vec<&str> = err.lines().rev().take(6).collect();
                emit("error", format!("Claude Code exited without an answer.\n{}", tail.into_iter().rev().collect::<Vec<_>>().join("\n")));
            }
        }
    });
    Ok(())
}
