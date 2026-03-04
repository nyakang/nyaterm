use crate::error::{AppError, AppResult};

use super::TranslateResult;

fn tk(text: &str, tkk: (i64, i64)) -> String {
    let mut a = tkk.0;
    let b = tkk.1;

    let bytes = text.as_bytes();
    let mut d: Vec<i64> = Vec::new();
    let mut e = 0usize;
    while e < bytes.len() {
        let mut g = i64::from(bytes[e]);
        if g < 128 {
            d.push(g);
        } else if g < 2048 {
            d.push(g >> 6 | 192);
            d.push(g & 63 | 128);
        } else if e + 1 < bytes.len()
            && (g & 0xfc00) == 0xd800
            && (i64::from(bytes[e + 1]) & 0xfc00) == 0xdc00
        {
            e += 1;
            g = 0x10000 + ((g & 0x3ff) << 10) + (i64::from(bytes[e]) & 0x3ff);
            d.push(g >> 18 | 240);
            d.push(g >> 12 & 63 | 128);
            d.push(g >> 6 & 63 | 128);
            d.push(g & 63 | 128);
        } else {
            d.push(g >> 12 | 224);
            d.push(g >> 6 & 63 | 128);
            d.push(g & 63 | 128);
        }
        e += 1;
    }

    a = a.wrapping_add(d.iter().sum::<i64>());

    fn rl(mut a: i64, b: &str) -> i64 {
        let chars: Vec<char> = b.chars().collect();
        let mut i = 0;
        while i < chars.len() - 2 {
            let d = chars[i + 2];
            let d_val = if d >= 'a' {
                i64::from(d as u32) - 87
            } else {
                i64::from(d.to_digit(10).unwrap_or(0))
            };
            let shifted = if chars[i + 1] == '+' {
                (a as u32) >> d_val as u32
            } else {
                (a as u32).wrapping_shl(d_val as u32)
            };
            a = if chars[i + 1] == '+' {
                i64::from(shifted)
            } else {
                (a as u32).wrapping_add(shifted) as i64
            };
            i += 3;
        }
        a
    }

    a = rl(a, "+-a^+6");
    a = rl(a, "+-3^+b+-f");
    a ^= b;
    if a < 0 {
        a = (a & 0x7fff_ffff) + 0x8000_0000;
    }
    a %= 1_000_000;
    format!("{a}.{}", a ^ tkk.0)
}

pub async fn translate(text: &str, target_lang: &str) -> AppResult<TranslateResult> {
    let client = reqwest::Client::new();

    let tkk = (444005, 2686318299i64);
    let tk_val = tk(text, tkk);

    let tl = google_lang(target_lang);

    let params = [
        ("client", "gtx"),
        ("sl", "auto"),
        ("tl", tl),
        ("hl", tl),
        ("dt", "t"),
        ("dt", "bd"),
        ("dj", "1"),
        ("source", "input"),
        ("tk", &tk_val),
        ("q", text),
    ];

    let resp = client
        .get("https://translate.googleapis.com/translate_a/single")
        .query(&params)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .send()
        .await
        .map_err(|e| AppError::Translation(format!("Google request failed: {e}")))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AppError::Translation(format!("Google parse failed: {e}")))?;

    let mut translated = String::new();
    if let Some(sentences) = body.get("sentences").and_then(|s| s.as_array()) {
        for sentence in sentences {
            if let Some(trans) = sentence.get("trans").and_then(|t| t.as_str()) {
                translated.push_str(trans);
            }
        }
    }

    let detected = body
        .get("src")
        .and_then(|s| s.as_str())
        .unwrap_or("auto")
        .to_string();

    if translated.is_empty() {
        return Err(AppError::Translation("Google returned empty translation".into()));
    }

    Ok(TranslateResult {
        original: text.to_string(),
        translated,
        detected_language: detected,
        provider: "google".to_string(),
    })
}

fn google_lang(lang: &str) -> &str {
    match lang {
        "zh-CN" | "zh_CN" => "zh-CN",
        "zh-TW" | "zh_TW" => "zh-TW",
        l => l,
    }
}
