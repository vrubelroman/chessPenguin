//! Hand-written parser for the engine → GUI subset of UCI that the supervisor
//! needs. Malformed lines produce `Err`, never a panic.

use crate::types::{EngineOption, EngineScore, OptionKind, Score, ScoreBound, SearchInfo};

/// One parsed line of engine output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineLine {
    IdName(String),
    IdAuthor(String),
    Option(EngineOption),
    UciOk,
    ReadyOk,
    BestMove {
        /// `None` for `bestmove (none)` / `bestmove 0000` (no legal moves).
        best: Option<String>,
        ponder: Option<String>,
    },
    Info(SearchInfo),
    /// `info string ...` without other search data.
    InfoString(String),
    /// Empty line.
    Empty,
    /// Anything else (banners, `copyprotection`, `registration`, vendor
    /// extensions). The supervisor ignores these during the handshake.
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}: {line:?}")]
pub struct ParseError {
    pub line: String,
    pub reason: String,
}

/// Tokens with their byte offsets so that "rest of line" values keep their
/// original spacing.
fn tokens(line: &str) -> Vec<(usize, &str)> {
    let base = line.as_ptr() as usize;
    line.split_ascii_whitespace()
        .map(|t| (t.as_ptr() as usize - base, t))
        .collect()
}

pub fn parse_line(line: &str) -> Result<EngineLine, ParseError> {
    let toks = tokens(line);
    let err = |reason: &str| ParseError {
        line: line.to_string(),
        reason: reason.to_string(),
    };
    let Some(&(_, first)) = toks.first() else {
        return Ok(EngineLine::Empty);
    };
    match first {
        "uciok" => Ok(EngineLine::UciOk),
        "readyok" => Ok(EngineLine::ReadyOk),
        "id" => match toks.get(1) {
            Some(&(_, "name")) | Some(&(_, "author")) => {
                let value = toks
                    .get(2)
                    .map(|&(off, _)| line[off..].trim_end().to_string())
                    .unwrap_or_default();
                if toks[1].1 == "name" {
                    Ok(EngineLine::IdName(value))
                } else {
                    Ok(EngineLine::IdAuthor(value))
                }
            }
            _ => Ok(EngineLine::Unknown(line.to_string())),
        },
        "bestmove" => {
            let best = toks.get(1).ok_or_else(|| err("bestmove without a move"))?.1;
            let best = match best {
                "(none)" | "0000" | "none" => None,
                m => Some(m.to_string()),
            };
            let ponder = match (toks.get(2), toks.get(3)) {
                (Some(&(_, "ponder")), Some(&(_, p))) if p != "(none)" && p != "0000" => {
                    Some(p.to_string())
                }
                _ => None,
            };
            Ok(EngineLine::BestMove { best, ponder })
        }
        "option" => parse_option(line, &toks)
            .map(EngineLine::Option)
            .map_err(|r| err(&r)),
        "info" => parse_info(line, &toks).map_err(|r| err(&r)),
        _ => Ok(EngineLine::Unknown(line.to_string())),
    }
}

const INFO_KEYS: &[&str] = &[
    "depth",
    "seldepth",
    "time",
    "nodes",
    "pv",
    "multipv",
    "score",
    "currmove",
    "currmovenumber",
    "hashfull",
    "nps",
    "tbhits",
    "sbhits",
    "cpuload",
    "string",
    "refutation",
    "currline",
    "wdl",
];

fn parse_info(line: &str, toks: &[(usize, &str)]) -> Result<EngineLine, String> {
    fn num<T: std::str::FromStr>(toks: &[(usize, &str)], i: usize, key: &str) -> Result<T, String> {
        let t = toks
            .get(i)
            .ok_or_else(|| format!("missing value for `{key}`"))?
            .1;
        t.parse::<T>()
            .map_err(|_| format!("bad value {t:?} for `{key}`"))
    }
    let mut info = SearchInfo::default();
    let mut string: Option<String> = None;
    let mut has_data = false;
    let mut i = 1;
    while i < toks.len() {
        let key = toks[i].1;
        i += 1;
        match key {
            "depth" => {
                info.depth = Some(num(toks, i, key)?);
                i += 1;
            }
            "seldepth" => {
                info.seldepth = Some(num(toks, i, key)?);
                i += 1;
            }
            "multipv" => {
                let v: u32 = num(toks, i, key)?;
                if v == 0 {
                    return Err("multipv must be >= 1".into());
                }
                info.multipv = Some(v);
                i += 1;
            }
            "nodes" => {
                info.nodes = Some(num(toks, i, key)?);
                i += 1;
            }
            "nps" => {
                info.nps = Some(num(toks, i, key)?);
                i += 1;
            }
            "time" => {
                info.time_ms = Some(num(toks, i, key)?);
                i += 1;
            }
            "hashfull" => {
                info.hashfull = Some(num(toks, i, key)?);
                i += 1;
            }
            "tbhits" => {
                info.tbhits = Some(num(toks, i, key)?);
                i += 1;
            }
            "wdl" => {
                let w = num(toks, i, key)?;
                let d = num(toks, i + 1, key)?;
                let l = num(toks, i + 2, key)?;
                info.wdl = Some((w, d, l));
                i += 3;
            }
            "score" => {
                let kind = toks.get(i).ok_or("score without type")?.1;
                let value: i32 = num(toks, i + 1, kind)?;
                let score = match kind {
                    "cp" => Score::Cp(value),
                    "mate" => Score::Mate(value),
                    other => return Err(format!("unknown score type {other:?}")),
                };
                i += 2;
                let mut bound = ScoreBound::Exact;
                while let Some(&(_, t)) = toks.get(i) {
                    match t {
                        "lowerbound" => bound = ScoreBound::Lower,
                        "upperbound" => bound = ScoreBound::Upper,
                        _ => break,
                    }
                    i += 1;
                }
                info.score = Some(EngineScore { score, bound });
            }
            "pv" => {
                while let Some(&(_, t)) = toks.get(i) {
                    if INFO_KEYS.contains(&t) {
                        break;
                    }
                    info.pv.push(t.to_string());
                    i += 1;
                }
            }
            "string" => {
                string = Some(
                    toks.get(i)
                        .map(|&(off, _)| line[off..].trim_end().to_string())
                        .unwrap_or_default(),
                );
                break;
            }
            // Fields we do not expose: skip their arguments.
            "currmove" | "currmovenumber" | "sbhits" | "cpuload" => {
                i += 1;
                continue;
            }
            "refutation" | "currline" => {
                while let Some(&(_, t)) = toks.get(i) {
                    if INFO_KEYS.contains(&t) {
                        break;
                    }
                    i += 1;
                }
                continue;
            }
            // Unknown vendor keys are ignored for forward compatibility.
            _ => continue,
        }
        has_data = true;
    }
    match (has_data, string) {
        (false, Some(s)) => Ok(EngineLine::InfoString(s)),
        _ => Ok(EngineLine::Info(info)),
    }
}

const OPTION_KEYS: &[&str] = &["default", "min", "max", "var"];

fn parse_option(line: &str, toks: &[(usize, &str)]) -> Result<EngineOption, String> {
    if toks.get(1).map(|t| t.1) != Some("name") {
        return Err("option without name".into());
    }
    let type_pos = toks
        .iter()
        .position(|&(_, t)| t == "type")
        .ok_or("option without type")?;
    if type_pos < 3 {
        return Err("empty option name".into());
    }
    let name = line[toks[2].0..toks[type_pos].0].trim().to_string();
    let kind_tok = toks.get(type_pos + 1).ok_or("option without type value")?.1;

    // Collect keyword → raw value slices after the type token.
    let mut default: Option<String> = None;
    let mut min: Option<&str> = None;
    let mut max: Option<&str> = None;
    let mut vars: Vec<String> = Vec::new();
    let mut i = type_pos + 2;
    while i < toks.len() {
        let key = toks[i].1;
        if !OPTION_KEYS.contains(&key) {
            return Err(format!("unexpected token {key:?} in option"));
        }
        let start = i + 1;
        let mut end = start;
        while end < toks.len() && !OPTION_KEYS.contains(&toks[end].1) {
            end += 1;
        }
        let value = if start < end {
            let from = toks[start].0;
            let to = toks[end - 1].0 + toks[end - 1].1.len();
            line[from..to].to_string()
        } else {
            String::new()
        };
        match key {
            "default" => default = Some(value),
            "min" => min = Some(toks.get(start).map(|t| t.1).unwrap_or("")),
            "max" => max = Some(toks.get(start).map(|t| t.1).unwrap_or("")),
            _ => vars.push(value),
        }
        i = end;
    }
    let int = |v: &str, what: &str| {
        v.parse::<i64>()
            .map_err(|_| format!("bad {what} {v:?} for option {name:?}"))
    };
    let kind = match kind_tok {
        "check" => OptionKind::Check {
            default: match default.as_deref() {
                Some("true") => true,
                Some("false") => false,
                other => return Err(format!("bad check default {other:?}")),
            },
        },
        "spin" => OptionKind::Spin {
            default: int(default.as_deref().ok_or("spin without default")?, "default")?,
            min: min.map(|v| int(v, "min")).transpose()?,
            max: max.map(|v| int(v, "max")).transpose()?,
        },
        "combo" => OptionKind::Combo {
            default: default.unwrap_or_default(),
            vars,
        },
        "button" => OptionKind::Button,
        "string" => OptionKind::String {
            default: default.unwrap_or_default(),
        },
        other => return Err(format!("unknown option type {other:?}")),
    };
    Ok(EngineOption { name, kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(line: &str) -> SearchInfo {
        match parse_line(line).unwrap() {
            EngineLine::Info(i) => i,
            other => panic!("not info: {other:?}"),
        }
    }

    #[test]
    fn stockfish_info_line() {
        let i = info(
            "info depth 10 seldepth 14 multipv 2 score cp -23 upperbound nodes 12345 nps 678900 \
             hashfull 12 tbhits 0 time 18 pv e7e5 g1f3 b8c6",
        );
        assert_eq!(i.depth, Some(10));
        assert_eq!(i.seldepth, Some(14));
        assert_eq!(i.multipv, Some(2));
        assert_eq!(
            i.score,
            Some(EngineScore {
                score: Score::Cp(-23),
                bound: ScoreBound::Upper
            })
        );
        assert_eq!(i.nodes, Some(12345));
        assert_eq!(i.nps, Some(678900));
        assert_eq!(i.hashfull, Some(12));
        assert_eq!(i.time_ms, Some(18));
        assert_eq!(i.pv, vec!["e7e5", "g1f3", "b8c6"]);
    }

    #[test]
    fn mate_and_wdl() {
        let i = info("info depth 5 score mate -3 wdl 0 0 1000 pv a1a2");
        assert_eq!(i.score.unwrap().score, Score::Mate(-3));
        assert_eq!(i.wdl, Some((0, 0, 1000)));
        assert_eq!(i.multipv_index(), 1);
        let z = info("info depth 0 score mate 0");
        assert_eq!(z.score.unwrap().score, Score::Mate(0));
        assert!(z.pv.is_empty());
    }

    #[test]
    fn pv_stops_at_known_key_and_lowerbound() {
        let i = info("info score cp 15 lowerbound pv e2e4 e7e5 depth 3");
        assert_eq!(i.pv, vec!["e2e4", "e7e5"]);
        assert_eq!(i.depth, Some(3));
        assert_eq!(i.score.unwrap().bound, ScoreBound::Lower);
    }

    #[test]
    fn info_string_and_currmove() {
        assert_eq!(
            parse_line("info string NNUE evaluation  using nn.nnue").unwrap(),
            EngineLine::InfoString("NNUE evaluation  using nn.nnue".into())
        );
        let i = info("info depth 3 currmove e2e4 currmovenumber 1");
        assert_eq!(i.depth, Some(3));
        assert!(i.pv.is_empty());
    }

    #[test]
    fn malformed_info_is_error() {
        for l in [
            "info depth x",
            "info depth",
            "info score banana 3",
            "info score cp",
            "info multipv 0",
            "info nodes -5",
            "info wdl 1 2",
        ] {
            assert!(parse_line(l).is_err(), "{l}");
        }
    }

    #[test]
    fn bestmove_variants() {
        assert_eq!(
            parse_line("bestmove e2e4 ponder c7c6").unwrap(),
            EngineLine::BestMove {
                best: Some("e2e4".into()),
                ponder: Some("c7c6".into())
            }
        );
        assert_eq!(
            parse_line("bestmove (none)").unwrap(),
            EngineLine::BestMove {
                best: None,
                ponder: None
            }
        );
        assert!(parse_line("bestmove").is_err());
    }

    #[test]
    fn id_lines_keep_spacing() {
        assert_eq!(
            parse_line("id name Stockfish 19").unwrap(),
            EngineLine::IdName("Stockfish 19".into())
        );
        assert_eq!(
            parse_line("id author the Stockfish developers (see AUTHORS file)").unwrap(),
            EngineLine::IdAuthor("the Stockfish developers (see AUTHORS file)".into())
        );
        assert_eq!(parse_line("   ").unwrap(), EngineLine::Empty);
        assert!(matches!(
            parse_line("Stockfish 19 by the Stockfish developers").unwrap(),
            EngineLine::Unknown(_)
        ));
    }

    fn opt(line: &str) -> EngineOption {
        match parse_line(line).unwrap() {
            EngineLine::Option(o) => o,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn options() {
        assert_eq!(
            opt("option name Threads type spin default 1 min 1 max 1024"),
            EngineOption {
                name: "Threads".into(),
                kind: OptionKind::Spin {
                    default: 1,
                    min: Some(1),
                    max: Some(1024)
                }
            }
        );
        assert_eq!(
            opt("option name Debug Log File type string default <empty>").kind,
            OptionKind::String {
                default: "<empty>".into()
            }
        );
        assert_eq!(
            opt("option name Book File type string default").kind,
            OptionKind::String { default: "".into() }
        );
        assert_eq!(
            opt("option name Ponder type check default false"),
            EngineOption {
                name: "Ponder".into(),
                kind: OptionKind::Check { default: false }
            }
        );
        assert_eq!(opt("option name Clear Hash type button").name, "Clear Hash");
        assert_eq!(
            opt("option name Play Style type combo default Normal var Solid var Normal var Very Risky")
                .kind,
            OptionKind::Combo {
                default: "Normal".into(),
                vars: vec!["Solid".into(), "Normal".into(), "Very Risky".into()]
            }
        );
        for bad in [
            "option name X type spin default x",
            "option name X type check default maybe",
            "option name X type",
            "option name type spin default 1",
            "option X type button",
            "option name X type weird",
        ] {
            assert!(parse_line(bad).is_err(), "{bad}");
        }
    }
}
