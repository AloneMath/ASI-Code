use std::fs;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceEngine {
    Local,
    OpenAi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyWaitOutcome {
    Triggered,
    Escaped,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyHoldOutcome {
    Captured(Option<String>),
    WaitTimedOut,
}

pub fn is_supported() -> bool {
    cfg!(target_os = "windows")
}

pub fn openai_key_present() -> bool {
    std::env::var("OPENAI_API_KEY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

pub fn parse_engine(raw: &str) -> Option<VoiceEngine> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "local" => Some(VoiceEngine::Local),
        "openai" => Some(VoiceEngine::OpenAi),
        _ => None,
    }
}

pub fn engine_name(engine: VoiceEngine) -> &'static str {
    match engine {
        VoiceEngine::Local => "local",
        VoiceEngine::OpenAi => "openai",
    }
}

pub fn auto_engine() -> VoiceEngine {
    if openai_key_present() {
        VoiceEngine::OpenAi
    } else {
        VoiceEngine::Local
    }
}

pub fn default_engine() -> VoiceEngine {
    if let Ok(v) = std::env::var("ASI_VOICE_ENGINE") {
        if v.trim().eq_ignore_ascii_case("auto") {
            return auto_engine();
        }
        if let Some(p) = parse_engine(&v) {
            return p;
        }
    }
    VoiceEngine::Local
}

pub fn openai_fallback_local_enabled() -> bool {
    parse_bool_env("ASI_VOICE_OPENAI_FALLBACK_LOCAL", true)
}

pub fn local_soft_fail_enabled() -> bool {
    parse_bool_env("ASI_VOICE_LOCAL_SOFT_FAIL", true)
}

pub fn parse_hotkey_name(raw: &str) -> Option<String> {
    let key = raw.trim().to_ascii_lowercase();
    if key.is_empty() {
        return None;
    }

    match key.as_str() {
        "space" | "spacebar" => return Some("Spacebar".to_string()),
        "esc" | "escape" => return Some("Escape".to_string()),
        "enter" | "return" => return Some("Enter".to_string()),
        "tab" => return Some("Tab".to_string()),
        "backspace" => return Some("Backspace".to_string()),
        "up" | "uparrow" => return Some("UpArrow".to_string()),
        "down" | "downarrow" => return Some("DownArrow".to_string()),
        "left" | "leftarrow" => return Some("LeftArrow".to_string()),
        "right" | "rightarrow" => return Some("RightArrow".to_string()),
        _ => {}
    }

    if key.len() == 1 {
        let ch = key.chars().next()?;
        if ch.is_ascii_alphabetic() {
            return Some(ch.to_ascii_uppercase().to_string());
        }
        if ch.is_ascii_digit() {
            return Some(format!("D{}", ch));
        }
    }

    if let Some(rest) = key.strip_prefix('f') {
        if let Ok(n) = rest.parse::<u8>() {
            if (1..=24).contains(&n) {
                return Some(format!("F{}", n));
            }
        }
    }

    None
}

pub fn normalize_hotkey_name(raw: &str) -> String {
    parse_hotkey_name(raw).unwrap_or_else(|| "F8".to_string())
}

pub fn wait_for_hotkey_once(hotkey: &str, timeout_secs: u64) -> Result<HotkeyWaitOutcome, String> {
    if !is_supported() {
        return Err("voice mode is supported on Windows only".to_string());
    }

    let canonical = parse_hotkey_name(hotkey).ok_or_else(|| {
        "invalid hotkey; supported: A-Z, 0-9, F1-F24, Space, Enter, Esc, Tab, arrows".to_string()
    })?;
    let timeout = timeout_secs.clamp(1, 300);
    let key_ps = ps_quote_literal(&canonical);

    let script = format!(
        "$ErrorActionPreference='Stop'; \
$target = {}; \
$deadline = (Get-Date).AddSeconds({}); \
while ((Get-Date) -lt $deadline) {{ \
  if ([Console]::KeyAvailable) {{ \
    $k = [Console]::ReadKey($true); \
    if ($k.Key -eq [System.ConsoleKey]::Escape) {{ 'escape'; exit 0 }} \
    if ($k.Key.ToString() -eq $target) {{ 'hotkey'; exit 0 }} \
  }} \
  Start-Sleep -Milliseconds 40; \
}} \
'timeout'",
        key_ps, timeout
    );

    let out = match run_powershell(&script) {
        Ok(v) => v,
        Err(e) => {
            let lower = e.to_ascii_lowercase();
            if lower.contains("keyavailable")
                && (lower.contains("redirect") || lower.contains("console input"))
            {
                return Err(
                    "hotkey-listen requires an interactive console (stdin cannot be redirected)"
                        .to_string(),
                );
            }
            return Err(e);
        }
    };
    match out.trim().to_ascii_lowercase().as_str() {
        "hotkey" => Ok(HotkeyWaitOutcome::Triggered),
        "escape" => Ok(HotkeyWaitOutcome::Escaped),
        "timeout" => Ok(HotkeyWaitOutcome::TimedOut),
        other => Err(format!("unexpected hotkey wait result: {}", other)),
    }
}

fn hotkey_vk_code(canonical: &str) -> Option<u16> {
    match canonical {
        "Spacebar" => Some(0x20),
        "Enter" => Some(0x0D),
        "Escape" => Some(0x1B),
        "Tab" => Some(0x09),
        "Backspace" => Some(0x08),
        "LeftArrow" => Some(0x25),
        "UpArrow" => Some(0x26),
        "RightArrow" => Some(0x27),
        "DownArrow" => Some(0x28),
        _ => {
            if canonical.len() == 1 {
                let ch = canonical.chars().next()?;
                if ch.is_ascii_uppercase() {
                    return Some(ch as u16);
                }
            }
            if let Some(rest) = canonical.strip_prefix('D') {
                if rest.len() == 1 {
                    let d = rest.chars().next()?;
                    if d.is_ascii_digit() {
                        return Some(d as u16);
                    }
                }
            }
            if let Some(rest) = canonical.strip_prefix('F') {
                if let Ok(n) = rest.parse::<u16>() {
                    if (1..=24).contains(&n) {
                        return Some(0x70 + (n - 1));
                    }
                }
            }
            None
        }
    }
}

pub fn recognize_while_hotkey_held(
    hotkey: &str,
    wait_key_secs: u64,
    max_hold_secs: u64,
) -> Result<HotkeyHoldOutcome, String> {
    if !is_supported() {
        return Err("voice mode is supported on Windows only".to_string());
    }

    let canonical = parse_hotkey_name(hotkey).ok_or_else(|| {
        "invalid hotkey; supported: A-Z, 0-9, F1-F24, Space, Enter, Esc, Tab, arrows".to_string()
    })?;
    let vk = hotkey_vk_code(&canonical)
        .ok_or_else(|| format!("hotkey {} is not supported for hold-listen", canonical))?;
    let wait_secs = wait_key_secs.clamp(1, 300);
    let hold_secs = max_hold_secs.clamp(1, 300);

    let script = format!(
        "$ErrorActionPreference='Stop'; \
Add-Type -AssemblyName System.Speech; \
Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class WinKeyState {{ [DllImport(\"user32.dll\")] public static extern short GetAsyncKeyState(int vKey); }}'; \
$vk = {}; \
$waitDeadline = (Get-Date).AddSeconds({}); \
while ((Get-Date) -lt $waitDeadline) {{ \
  if (([WinKeyState]::GetAsyncKeyState($vk) -band 0x8000) -ne 0) {{ break }} \
  Start-Sleep -Milliseconds 25; \
}} \
if (([WinKeyState]::GetAsyncKeyState($vk) -band 0x8000) -eq 0) {{ 'timeout_wait_key'; exit 0 }} \
$script:asi_text = ''; \
$rec = New-Object System.Speech.Recognition.SpeechRecognitionEngine; \
$rec.SetInputToDefaultAudioDevice(); \
$rec.LoadGrammar((New-Object System.Speech.Recognition.DictationGrammar)); \
$rec.add_SpeechRecognized({{ param($sender,$e) if ($null -ne $e.Result -and -not [string]::IsNullOrWhiteSpace($e.Result.Text)) {{ if ([string]::IsNullOrWhiteSpace($script:asi_text)) {{ $script:asi_text = $e.Result.Text }} else {{ $script:asi_text = $script:asi_text + ' ' + $e.Result.Text }} }} }}); \
$rec.RecognizeAsync([System.Speech.Recognition.RecognizeMode]::Multiple); \
$holdDeadline = (Get-Date).AddSeconds({}); \
while ((Get-Date) -lt $holdDeadline) {{ \
  if (([WinKeyState]::GetAsyncKeyState($vk) -band 0x8000) -eq 0) {{ break }} \
  Start-Sleep -Milliseconds 30; \
}} \
$rec.RecognizeAsyncStop(); \
Start-Sleep -Milliseconds 350; \
$rec.Dispose(); \
if ([string]::IsNullOrWhiteSpace($script:asi_text)) {{ '' }} else {{ $script:asi_text.Trim() }}",
        vk,
        wait_secs,
        hold_secs
    );

    let out = match run_powershell(&script) {
        Ok(v) => v,
        Err(e) => {
            let mapped = normalize_voice_error(&e);
            let lower = mapped.to_ascii_lowercase();
            if lower.contains("redirect") || lower.contains("console input") {
                return Err(
                    "hold-listen requires an interactive console (stdin cannot be redirected)"
                        .to_string(),
                );
            }
            return Err(mapped);
        }
    };

    let trimmed = out.trim();
    if trimmed.eq_ignore_ascii_case("timeout_wait_key") {
        return Ok(HotkeyHoldOutcome::WaitTimedOut);
    }
    if trimmed.is_empty() {
        return Ok(HotkeyHoldOutcome::Captured(None));
    }

    Ok(HotkeyHoldOutcome::Captured(Some(trimmed.to_string())))
}

pub fn recognize_once(timeout_secs: u64) -> Result<Option<String>, String> {
    if !is_supported() {
        return Err("voice mode is supported on Windows only".to_string());
    }

    let timeout = timeout_secs.clamp(1, 120);
    let script = format!(
        "$ErrorActionPreference='Stop'; \
Add-Type -AssemblyName System.Speech; \
$rec = New-Object System.Speech.Recognition.SpeechRecognitionEngine; \
$rec.SetInputToDefaultAudioDevice(); \
$rec.LoadGrammar((New-Object System.Speech.Recognition.DictationGrammar)); \
$res = $rec.Recognize([TimeSpan]::FromSeconds({})); \
if ($null -eq $res) {{ '' }} else {{ $res.Text }}",
        timeout
    );

    let out = run_powershell(&script).map_err(|e| normalize_voice_error(&e))?;
    let text = out.trim().to_string();
    if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}

pub fn speak_text_with_engine(
    text: &str,
    engine: VoiceEngine,
    openai_voice: &str,
) -> Result<(), String> {
    match engine {
        VoiceEngine::Local => speak_text_local(text),
        VoiceEngine::OpenAi => match speak_text_openai(text, openai_voice) {
            Ok(()) => Ok(()),
            Err(openai_err) => {
                if !openai_fallback_local_enabled() {
                    return Err(format!("openai_tts_failed: {}", openai_err));
                }

                match speak_text_local(text) {
                    Ok(()) => Ok(()),
                    Err(local_err) => Err(format!(
                        "openai_tts_failed: {}; local_fallback_failed: {}",
                        openai_err, local_err
                    )),
                }
            }
        },
    }
}

fn speak_text_local(text: &str) -> Result<(), String> {
    if !is_supported() {
        return Err("voice mode is supported on Windows only".to_string());
    }

    let cleaned = text.trim();
    if cleaned.is_empty() {
        return Ok(());
    }

    let clipped = clip_chars(cleaned, 1200);
    let path = temp_voice_file("tts_local", "txt");
    fs::write(&path, clipped.as_bytes()).map_err(|e| format!("write {}: {}", path.display(), e))?;

    let path_ps = ps_quote_literal(&path.to_string_lossy());
    let script = format!(
        "$ErrorActionPreference='Stop'; \
Add-Type -AssemblyName System.Speech; \
$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
$synth.SetOutputToDefaultAudioDevice(); \
$text = Get-Content -LiteralPath {} -Raw; \
if (-not [string]::IsNullOrWhiteSpace($text)) {{ $synth.Speak($text) }}",
        path_ps
    );

    let result = run_powershell(&script).map(|_| ());
    let _ = fs::remove_file(&path);

    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            if local_soft_fail_enabled() && is_likely_non_interactive_audio_error(&e) {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

fn speak_text_openai(text: &str, openai_voice: &str) -> Result<(), String> {
    if !is_supported() {
        return Err("voice mode is supported on Windows only".to_string());
    }

    let cleaned = text.trim();
    if cleaned.is_empty() {
        return Ok(());
    }

    let key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| "OPENAI_API_KEY is required for openai voice engine".to_string())?;
    if key.trim().is_empty() {
        return Err("OPENAI_API_KEY is required for openai voice engine".to_string());
    }

    let endpoint = std::env::var("ASI_VOICE_OPENAI_TTS_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1/audio/speech".to_string());
    let model = std::env::var("ASI_VOICE_OPENAI_TTS_MODEL")
        .unwrap_or_else(|_| "gpt-4o-mini-tts".to_string());

    let voice_name = if openai_voice.trim().is_empty() {
        "alloy"
    } else {
        openai_voice.trim()
    };

    let payload = json!({
        "model": model,
        "voice": voice_name,
        "input": clip_chars(cleaned, 1200),
        "format": "wav"
    });

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("build tts client: {}", e))?;

    let resp = client
        .post(&endpoint)
        .header("Authorization", format!("Bearer {}", key.trim()))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .map_err(|e| format!("openai tts request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        let body_short = clip_chars(body.trim(), 500);
        return Err(format!("openai tts http {}: {}", status, body_short));
    }

    let bytes = resp
        .bytes()
        .map_err(|e| format!("read tts audio body failed: {}", e))?;

    let wav_path = temp_voice_file("tts_openai", "wav");
    fs::write(&wav_path, &bytes).map_err(|e| format!("write {}: {}", wav_path.display(), e))?;

    let path_ps = ps_quote_literal(&wav_path.to_string_lossy());
    let script = format!(
        "$ErrorActionPreference='Stop'; \
$player = New-Object System.Media.SoundPlayer {}; \
$player.Load(); \
$player.PlaySync();",
        path_ps
    );

    let result = run_powershell(&script).map(|_| ());
    let _ = fs::remove_file(&wav_path);
    result
}

fn run_powershell(script: &str) -> Result<String, String> {
    let output = ProcessCommand::new("powershell")
        .args(["-NoProfile", "-Command", script])
        .output()
        .map_err(|e| format!("spawn powershell: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        Ok(stdout)
    } else {
        let msg = if !stderr.trim().is_empty() {
            stderr
        } else if !stdout.trim().is_empty() {
            stdout
        } else {
            format!("powershell exited with status {}", output.status)
        };
        Err(msg.trim().to_string())
    }
}

fn temp_voice_file(prefix: &str, ext: &str) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("asi_{}_{}.{}", prefix, ts, ext))
}

fn ps_quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn clip_chars(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, ch) in text.chars().enumerate() {
        if i >= max_chars {
            break;
        }
        out.push(ch);
    }
    out
}

fn is_likely_non_interactive_audio_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("object reference not set")
        || m.contains("nullreferenceexception")
        || m.contains("audio device")
        || m.contains("no installed audio output")
        || m.contains("setoutputtodefaultaudiodevice")
        || m.contains("speech synthesizer")
}

fn normalize_voice_error(msg: &str) -> String {
    let trimmed = msg.trim();
    let lower = trimmed.to_ascii_lowercase();

    if lower.contains("setinputtodefaultaudiodevice") && lower.contains("access is denied") {
        return "microphone access denied (E_ACCESSDENIED). Enable microphone access for desktop apps and PowerShell/terminal, then retry.".to_string();
    }

    if lower.contains("speechrecognized") && lower.contains("cannot be found") {
        return "speech recognition event binding failed in PowerShell (SpeechRecognized not found).".to_string();
    }

    if lower.contains("here-string header") && lower.contains("add-type @\"") {
        return "powershell parse error in voice script (invalid here-string header).".to_string();
    }

    trimmed.to_string()
}

fn parse_bool_env(key: &str, default_value: bool) -> bool {
    match std::env::var(key) {
        Ok(v) => match v.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "yes" => true,
            "0" | "false" | "off" | "no" => false,
            _ => default_value,
        },
        Err(_) => default_value,
    }
}
#[cfg(test)]
mod tests {
    use super::normalize_voice_error;

    #[test]
    fn normalize_voice_error_maps_mic_access_denied() {
        let raw = "Exception calling \"SetInputToDefaultAudioDevice\" with \"0\" argument(s): \"Access is denied.\"";
        let out = normalize_voice_error(raw);
        assert!(out.contains("microphone access denied"));
    }

    #[test]
    fn normalize_voice_error_maps_speechrecognized_binding() {
        let raw =
            "The property 'SpeechRecognized' cannot be found on this object. Verify that the property exists and can be set.";
        let out = normalize_voice_error(raw);
        assert!(out.contains("event binding failed"));
    }

    #[test]
    fn normalize_voice_error_passes_through_other_messages() {
        let raw = "plain failure";
        let out = normalize_voice_error(raw);
        assert_eq!(out, "plain failure");
    }
}

