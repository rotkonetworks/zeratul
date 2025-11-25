//! magic wormhole file transfer integration
//! sends wormhole codes via zcash memos for private file sharing
//! uses wormhole-rs CLI for simplicity

use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

/// wormhole transfer state
#[derive(Debug, Clone)]
pub enum TransferState {
    Idle,
    Sending { file: PathBuf, progress: f32 },
    WaitingForCode,
    CodeReady(String),
    Receiving { code: String, progress: f32 },
    Complete,
    Failed(String),
}

impl Default for TransferState {
    fn default() -> Self {
        TransferState::Idle
    }
}

/// wormhole code prefix for memos
pub const WORMHOLE_PREFIX: &str = "wormhole://";

/// parse wormhole code from memo
pub fn parse_wormhole_memo(memo: &str) -> Option<String> {
    memo.lines()
        .find(|line| line.starts_with(WORMHOLE_PREFIX))
        .map(|line| line[WORMHOLE_PREFIX.len()..].trim().to_string())
}

/// format wormhole code as memo
pub fn format_wormhole_memo(code: &str) -> String {
    format!("{}{}", WORMHOLE_PREFIX, code)
}

#[derive(Debug, Clone)]
pub enum TransferProgress {
    Started,
    CodeGenerated(String),
    Progress(f32),
    Complete(PathBuf),
    Failed(String),
}

/// file transfer using wormhole CLI
pub struct WormholeTransfer;

impl WormholeTransfer {
    /// send a file and get the wormhole code via CLI
    /// returns channel that will receive the code when ready
    pub async fn send_file(path: PathBuf) -> Result<(String, mpsc::Receiver<TransferProgress>)> {
        let (tx, rx) = mpsc::channel(32);

        tx.send(TransferProgress::Started).await.ok();

        // try wormhole-rs first, then fall back to magic-wormhole
        let wormhole_cmd = if Command::new("wormhole-rs").arg("--version").output().await.is_ok() {
            "wormhole-rs"
        } else if Command::new("wormhole").arg("--version").output().await.is_ok() {
            "wormhole"
        } else {
            return Err(anyhow!("wormhole not installed. install with: cargo install magic-wormhole-cli"));
        };

        let path_str = path.to_string_lossy().to_string();

        // spawn wormhole send process
        let mut child = Command::new(wormhole_cmd)
            .arg("send")
            .arg(&path_str)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
        let mut reader = BufReader::new(stderr).lines();

        let tx_clone = tx.clone();

        // parse output for the code
        tokio::spawn(async move {
            let mut code_found = None;

            while let Ok(Some(line)) = reader.next_line().await {
                // look for the wormhole code in output
                // format is typically: "wormhole receive X-word-word"
                if line.contains("wormhole receive") {
                    if let Some(code_start) = line.find("receive") {
                        let code_part = &line[code_start + 8..];
                        let code = code_part.trim().to_string();
                        if !code.is_empty() {
                            code_found = Some(code.clone());
                            tx_clone.send(TransferProgress::CodeGenerated(code)).await.ok();
                        }
                    }
                }
                // look for code pattern directly (number-word-word)
                else if let Some(code) = extract_wormhole_code(&line) {
                    code_found = Some(code.clone());
                    tx_clone.send(TransferProgress::CodeGenerated(code)).await.ok();
                }
            }

            // wait for process to complete
            match child.wait().await {
                Ok(status) if status.success() => {
                    if let Some(code) = code_found {
                        tx_clone.send(TransferProgress::Complete(PathBuf::from(&path_str))).await.ok();
                    }
                }
                Ok(_) => {
                    tx_clone.send(TransferProgress::Failed("transfer failed".into())).await.ok();
                }
                Err(e) => {
                    tx_clone.send(TransferProgress::Failed(e.to_string())).await.ok();
                }
            }
        });

        // wait briefly for code to be generated
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // try to get the code from a temp file or output
        // for now return placeholder - real implementation would parse from stderr
        Ok(("pending...".to_string(), rx))
    }

    /// receive a file using a wormhole code
    pub async fn receive_file(code: &str, download_dir: PathBuf) -> Result<mpsc::Receiver<TransferProgress>> {
        let (tx, rx) = mpsc::channel(32);

        tx.send(TransferProgress::Started).await.ok();

        let wormhole_cmd = if Command::new("wormhole-rs").arg("--version").output().await.is_ok() {
            "wormhole-rs"
        } else if Command::new("wormhole").arg("--version").output().await.is_ok() {
            "wormhole"
        } else {
            return Err(anyhow!("wormhole not installed"));
        };

        let code = code.to_string();
        let download_dir_clone = download_dir.clone();

        tokio::spawn(async move {
            let result = Command::new(wormhole_cmd)
                .arg("receive")
                .arg(&code)
                .arg("--accept-file")
                .current_dir(&download_dir_clone)
                .output()
                .await;

            match result {
                Ok(output) if output.status.success() => {
                    tx.send(TransferProgress::Complete(download_dir_clone)).await.ok();
                }
                Ok(output) => {
                    let err = String::from_utf8_lossy(&output.stderr);
                    tx.send(TransferProgress::Failed(err.to_string())).await.ok();
                }
                Err(e) => {
                    tx.send(TransferProgress::Failed(e.to_string())).await.ok();
                }
            }
        });

        Ok(rx)
    }
}

/// extract wormhole code from text (pattern: number-word-word)
fn extract_wormhole_code(text: &str) -> Option<String> {
    // look for pattern like "3-hesitate-dashboard"
    for word in text.split_whitespace() {
        let parts: Vec<&str> = word.split('-').collect();
        if parts.len() >= 2 {
            // check if first part is a number
            if parts[0].parse::<u32>().is_ok() {
                // check if remaining parts are words (alphabetic)
                if parts[1..].iter().all(|p| p.chars().all(|c| c.is_alphabetic())) {
                    return Some(word.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_wormhole_memo() {
        assert_eq!(
            parse_wormhole_memo("wormhole://3-hesitate-dashboard"),
            Some("3-hesitate-dashboard".to_string())
        );
        assert_eq!(
            parse_wormhole_memo("hello\nwormhole://5-test-code\nbye"),
            Some("5-test-code".to_string())
        );
        assert_eq!(
            parse_wormhole_memo("regular memo"),
            None
        );
    }

    #[test]
    fn test_extract_code() {
        assert_eq!(
            extract_wormhole_code("On the other computer, run: wormhole receive 3-hesitate-dashboard"),
            Some("3-hesitate-dashboard".to_string())
        );
        assert_eq!(
            extract_wormhole_code("no code here"),
            None
        );
    }
}
