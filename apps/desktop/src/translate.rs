//! Translating game annotations into the reader's language (docs/design/translation.md).
//!
//! Only the text of comments goes to the language model: board commands are cut out by
//! `notation::comment_text`, moves stay in the text and must come back unchanged (a
//! translation that loses or alters a move is not shown). Translations live in a cache
//! beside the program's other caches, never in the PGN.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write as _};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chess_core::{Game, NodeId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::i18n::Lang;

// --- Connections -----------------------------------------------------------------------

/// How the program reaches a language model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Connection {
    /// The user's ChatGPT subscription (Sign in with ChatGPT).
    #[default]
    ChatGpt,
    /// A service with an OpenAI-compatible API: a key, or a model on this computer.
    Key,
}

/// Services with an OpenAI-compatible API.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Preset {
    #[default]
    DeepSeek,
    OpenAi,
    Gemini,
    Ollama,
    LmStudio,
    Custom,
}

impl Preset {
    pub const ALL: [Preset; 6] = [
        Preset::DeepSeek,
        Preset::OpenAi,
        Preset::Gemini,
        Preset::Ollama,
        Preset::LmStudio,
        Preset::Custom,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Preset::DeepSeek => "DeepSeek",
            Preset::OpenAi => "OpenAI",
            Preset::Gemini => "Google Gemini",
            Preset::Ollama => "Ollama",
            Preset::LmStudio => "LM Studio",
            Preset::Custom => crate::i18n::tr("Another address"),
        }
    }

    /// The id the key and the consent are kept under.
    pub fn id(self) -> &'static str {
        match self {
            Preset::DeepSeek => "deepseek",
            Preset::OpenAi => "openai",
            Preset::Gemini => "gemini",
            Preset::Ollama => "ollama",
            Preset::LmStudio => "lmstudio",
            Preset::Custom => "custom",
        }
    }

    pub fn base(self) -> &'static str {
        match self {
            Preset::DeepSeek => "https://api.deepseek.com",
            Preset::OpenAi => "https://api.openai.com/v1",
            Preset::Gemini => "https://generativelanguage.googleapis.com/v1beta/openai",
            Preset::Ollama => "http://127.0.0.1:11434/v1",
            Preset::LmStudio => "http://127.0.0.1:1234/v1",
            Preset::Custom => "",
        }
    }

    /// The model used until the user picks another one ("" — the first the service lists).
    pub fn model(self) -> &'static str {
        match self {
            Preset::DeepSeek => "deepseek-chat",
            Preset::Gemini => "gemini-2.5-flash",
            Preset::Ollama => crate::translate_ui::OLLAMA_MODEL,
            _ => "",
        }
    }

    /// Where to get a key (none for a model on this computer).
    pub fn key_page(self) -> Option<&'static str> {
        match self {
            Preset::DeepSeek => Some("https://platform.deepseek.com/api_keys"),
            Preset::OpenAi => Some("https://platform.openai.com/api-keys"),
            Preset::Gemini => Some("https://aistudio.google.com/apikey"),
            _ => None,
        }
    }

    /// The service's privacy policy (none for a model on this computer or another address).
    pub fn privacy(self) -> Option<&'static str> {
        match self {
            Preset::DeepSeek => {
                Some("https://cdn.deepseek.com/policies/en-US/deepseek-privacy-policy.html")
            }
            Preset::OpenAi => Some("https://openai.com/policies/privacy-policy/"),
            Preset::Gemini => Some("https://ai.google.dev/gemini-api/terms"),
            _ => None,
        }
    }

    pub fn needs_key(self) -> bool {
        self.key_page().is_some()
    }

    /// `response_format: json_object` is understood (the others may reject it).
    fn json_mode(self) -> bool {
        matches!(self, Preset::DeepSeek | Preset::OpenAi | Preset::Gemini)
    }
}

/// The translation part of the settings.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TranslationSettings {
    /// The language to translate into (`None`: the interface language).
    pub target: Option<Lang>,
    pub connection: Connection,
    pub preset: Preset,
    /// The address of `Preset::Custom`, or another address of a preset.
    pub base: String,
    /// The model chosen for the key connection ("" — the preset's).
    pub model: String,
    /// The model chosen for ChatGPT ("" — picked from the list after sign-in).
    pub chatgpt_model: String,
    /// Do not offer translations at all.
    pub never_offer: bool,
    /// The user agreed once: comments in another language are translated right away.
    pub auto: bool,
    /// Services the user allowed to receive comments (`Preset::id`, "chatgpt").
    pub allowed: Vec<String>,
    /// Sign in with ChatGPT: the stable id of this installation, the client id issued at
    /// the first sign-in, and the account shown in the settings.
    pub host_id: String,
    pub client_id: String,
    pub account: String,
    /// How many times the offer was closed (from the second time: "Don't offer").
    pub declined: u32,
}

impl TranslationSettings {
    pub fn target(&self) -> Lang {
        self.target.unwrap_or_else(crate::i18n::current)
    }

    pub fn base(&self) -> String {
        let b = if self.base.trim().is_empty() {
            self.preset.base()
        } else {
            self.base.trim()
        };
        b.trim_end_matches('/').to_owned()
    }

    pub fn model(&self) -> String {
        if self.model.trim().is_empty() {
            self.preset.model().to_owned()
        } else {
            self.model.trim().to_owned()
        }
    }

    /// The id consent is given for.
    pub fn service_id(&self) -> String {
        match self.connection {
            Connection::ChatGpt => "chatgpt".into(),
            Connection::Key if self.is_local() => "local".into(),
            Connection::Key if self.preset == Preset::Custom => format!("custom:{}", self.base()),
            Connection::Key => self.preset.id().into(),
        }
    }

    /// The name of the service shown to the user.
    pub fn service_name(&self) -> String {
        match self.connection {
            Connection::ChatGpt => "ChatGPT".into(),
            Connection::Key if self.preset == Preset::Custom => host_of(&self.base()),
            Connection::Key => self.preset.name().into(),
        }
    }

    /// The model runs on this computer: no consent asked.
    pub fn is_local(&self) -> bool {
        self.connection == Connection::Key && is_local_address(&self.base())
    }

    pub fn allowed(&self) -> bool {
        self.is_local() || self.allowed.contains(&self.service_id())
    }

    /// Everything needed to send a request is there.
    pub fn ready(&self) -> bool {
        match self.connection {
            Connection::ChatGpt => credentials().chatgpt.is_some(),
            Connection::Key => {
                !self.base().is_empty()
                    && (!self.preset.needs_key() || credentials().key(self.preset.id()).is_some())
            }
        }
    }
}

pub fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    rest.split('/').next().unwrap_or(rest).to_owned()
}

pub fn is_local_address(url: &str) -> bool {
    let host = host_of(url);
    let host = host
        .rsplit_once(':')
        .filter(|(h, p)| !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        .map_or(host.as_str(), |(h, _)| h);
    matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1")
}

// --- Credentials ---------------------------------------------------------------------------

/// Keys and ChatGPT tokens, apart from the settings, in a file only the user can read
/// (as the Codex CLI keeps its sign-in).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Credentials {
    /// API keys by `Preset::id`.
    pub keys: HashMap<String, String>,
    pub chatgpt: Option<crate::chatgpt::Tokens>,
}

impl Credentials {
    pub fn key(&self, id: &str) -> Option<&str> {
        self.keys
            .get(id)
            .map(|k| k.trim())
            .filter(|k| !k.is_empty())
    }
}

static CREDENTIALS: Mutex<Option<Credentials>> = Mutex::new(None);

fn credentials_file() -> Option<PathBuf> {
    crate::settings::Settings::config_dir().map(|d| d.join("credentials.json"))
}

pub fn credentials() -> Credentials {
    let mut c = CREDENTIALS.lock().unwrap_or_else(|e| e.into_inner());
    c.get_or_insert_with(|| {
        credentials_file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    })
    .clone()
}

/// Changes the credentials and writes them, under one lock (a token refresh in the
/// background and a key typed in the settings must not undo each other).
pub fn update_credentials(change: impl FnOnce(&mut Credentials)) {
    let mut guard = CREDENTIALS.lock().unwrap_or_else(|e| e.into_inner());
    let c = guard.get_or_insert_with(|| {
        credentials_file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    });
    change(c);
    let c = c.clone();
    if let Some(f) = credentials_file() {
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(text) = serde_json::to_string_pretty(&c) {
            let tmp = f.with_extension("json.tmp");
            if write_private(&tmp, &text).is_ok() {
                let _ = std::fs::rename(&tmp, &f);
            }
        }
    }
}

fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    // A file left from before keeps its mode on open: set it again.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(text.as_bytes())
}

// --- Which comments to translate --------------------------------------------------------

/// The comments of a game in reading order (the text the notation shows), each once.
pub fn comments(game: &Game) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |c: &[String]| {
        let t = crate::notation::comment_text(c);
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    };
    let mut ids = vec![NodeId::ROOT];
    ids.extend(game.attached_nodes());
    for id in ids {
        if let Some(n) = game.node(id) {
            push(n.comments_before());
            push(n.comments_after());
        }
    }
    out
}

/// Letters of the words that are not moves, squares or numbers: what tells the language.
fn prose(text: &str) -> String {
    text.split_whitespace()
        .filter(|w| moves_in(w).is_empty())
        .flat_map(|w| w.chars().filter(|c| c.is_alphabetic()).chain([' ']))
        .collect()
}

/// The languages comments are told apart in (others are rare in chess texts and make
/// short English look like Afrikaans or Tagalog).
const LANGUAGES: &[whatlang::Lang] = {
    use whatlang::Lang::*;
    &[
        Eng, Rus, Deu, Fra, Spa, Ita, Por, Nld, Ukr, Bel, Pol, Ces, Slk, Slv, Hrv, Srp, Bul, Mkd,
        Ron, Hun, Ell, Tur, Swe, Dan, Nob, Fin, Est, Lav, Lit, Cat, Epo, Lat, Kat, Hye, Aze, Heb,
        Ara, Pes, Cmn, Jpn, Kor, Vie, Hin, Ind, Uzb,
    ]
};

fn detect(text: &str) -> Option<whatlang::Lang> {
    static DETECTOR: std::sync::OnceLock<whatlang::Detector> = std::sync::OnceLock::new();
    DETECTOR
        .get_or_init(|| whatlang::Detector::with_allowlist(LANGUAGES.to_vec()))
        .detect(text)
        .map(|i| i.lang())
}

/// The language as the detector knows it (`None`: it does not tell Kazakh, so every
/// comment counts as another language).
pub fn language_of(lang: Lang) -> Option<whatlang::Lang> {
    Some(match lang {
        Lang::En => whatlang::Lang::Eng,
        Lang::Ru => whatlang::Lang::Rus,
        Lang::Fr => whatlang::Lang::Fra,
        Lang::De => whatlang::Lang::Deu,
        Lang::Es => whatlang::Lang::Spa,
        Lang::It => whatlang::Lang::Ita,
        Lang::Pt => whatlang::Lang::Por,
        Lang::Uz => whatlang::Lang::Uzb,
        Lang::Kk => return None,
    })
}

/// What a game's comments need.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The language of most of the foreign text.
    pub source: whatlang::Lang,
    /// The comments to translate.
    pub texts: Vec<String>,
    /// All the comments with text.
    pub total: usize,
}

/// A comment shorter than this (in letters) takes the language of the game.
const SHORT: usize = 25;

/// The comments not in `target`, if there are enough of them to offer a translation:
/// three comments or 150 letters. Short ones ("Weak.", "Better was") follow the language
/// of the game.
pub fn plan(texts: &[String], target: Lang) -> Option<Plan> {
    let target = language_of(target);
    let mut letters: HashMap<whatlang::Lang, usize> = HashMap::new();
    let detected: Vec<(usize, Option<whatlang::Lang>)> = texts
        .iter()
        .map(|t| {
            let p = prose(t);
            let n = p.chars().filter(|c| c.is_alphabetic()).count();
            let lang = (n >= SHORT).then(|| detect(&p)).flatten();
            if let Some(l) = lang {
                *letters.entry(l).or_default() += n;
            }
            (n, lang)
        })
        .collect();
    let mut game_lang = letters.iter().max_by_key(|(_, n)| **n).map(|(l, _)| *l);
    // Only short comments: their text together tells the language.
    if game_lang.is_none() {
        let all: String = texts.iter().map(|t| prose(t)).collect::<Vec<_>>().join(" ");
        if all.chars().filter(|c| c.is_alphabetic()).count() >= SHORT {
            // Short phrases glued together: only the usual languages of chess books.
            use whatlang::Lang::*;
            game_lang = whatlang::Detector::with_allowlist(vec![Eng, Rus, Deu, Fra, Spa, Ita])
                .detect(&all)
                .map(|i| i.lang());
            if let Some(l) = game_lang {
                letters.insert(l, all.chars().filter(|c| c.is_alphabetic()).count());
            }
        }
    }
    let foreign: Vec<usize> = detected
        .iter()
        .enumerate()
        .filter(|(_, (n, lang))| *n > 0 && lang.or(game_lang).is_some_and(|l| Some(l) != target))
        .map(|(i, _)| i)
        .collect();
    let foreign_letters: usize = foreign.iter().map(|&i| detected[i].0).sum();
    if foreign.len() < 3 && foreign_letters < 60 {
        return None;
    }
    let source = letters
        .iter()
        .filter(|(l, _)| Some(**l) != target)
        .max_by_key(|(_, n)| **n)
        .map(|(l, _)| *l)?;
    Some(Plan {
        source,
        texts: foreign.into_iter().map(|i| texts[i].clone()).collect(),
        total: texts.len(),
    })
}

// --- Moves must come back unchanged --------------------------------------------------------

/// Piece letters a move may start with: Latin, figurines, and the Russian ones (a
/// translation must not swap one for another).
const PIECES: &[&str] = &[
    "Кр", "K", "Q", "R", "B", "N", "♔", "♕", "♖", "♗", "♘", "♚", "♛", "♜", "♝", "♞", "Ф", "Л", "С",
    "К",
];

fn file(c: char) -> bool {
    ('a'..='h').contains(&c)
}

fn rank(c: char) -> bool {
    ('1'..='8').contains(&c)
}

/// A move or a square starting at `c[i]` (a word boundary): its length in chars.
fn move_at(c: &[char], i: usize) -> Option<usize> {
    let rest: String = c[i..c.len().min(i + 7)].iter().collect();
    for castle in ["O-O-O", "0-0-0", "O-O", "0-0"] {
        if rest.starts_with(castle) {
            return Some(castle.chars().count());
        }
    }
    let mut j = i;
    if let Some(p) = PIECES.iter().find(|p| rest.starts_with(**p)) {
        j += p.chars().count();
    }
    // [file][rank][x] before the target square: the longest form that ends on a square.
    let at = |k: usize| c.get(k).copied().unwrap_or(' ');
    let square_at = |k: usize| file(at(k)) && rank(at(k + 1));
    let mut found = None;
    for len in [2usize, 1, 0] {
        let ok = match len {
            2 => file(at(j)) && rank(at(j + 1)),
            1 => file(at(j)) || rank(at(j)),
            _ => true,
        };
        if !ok {
            continue;
        }
        let mut k = j + len;
        if at(k) == 'x' {
            k += 1;
        }
        // Without a capture, a lone square is the move itself (len 0), not a disambiguation.
        if square_at(k) && (len == 0 || at(j + len) == 'x' || len < 2 || square_at(j + 2)) {
            found = Some(k);
            break;
        }
    }
    let mut k = found? + 2;
    // Promotion: "=Q", or "Q" right after the last rank ("e8Q").
    if at(k) == '=' && "QRBN".contains(at(k + 1)) {
        k += 2;
    } else if "QRBN".contains(at(k))
        && !at(k + 1).is_alphanumeric()
        && matches!(at(k - 1), '1' | '8')
    {
        k += 1;
    }
    Some(k - i)
}

/// The moves and squares of a text, normalized ("0-0" → "O-O", "e8Q" → "e8=Q") and
/// sorted. Words are cut at any non-letter: "a4-a5", "Nf3/Ng5", "the e5-square",
/// "12.Nf3," all give their moves.
fn moves_in(text: &str) -> Vec<String> {
    let c: Vec<char> = text.chars().collect();
    let mut v = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let boundary = i == 0 || !c[i - 1].is_alphabetic();
        if boundary && let Some(n) = move_at(&c, i) {
            let end = i + n;
            if c.get(end).is_none_or(|x| !x.is_alphanumeric()) {
                let mut m: String = c[i..end].iter().collect();
                m = m.replace('0', "O");
                if let Some(q) = m.chars().last()
                    && "QRBN".contains(q)
                    && !m.contains('=')
                    && m.chars().count() >= 3
                    && m.chars().rev().nth(1).is_some_and(|r| r == '1' || r == '8')
                {
                    m.insert(m.len() - 1, '=');
                }
                v.push(m);
                i = end;
                continue;
            }
        }
        i += 1;
    }
    v.sort();
    v
}

/// A translation keeps every move and square of the original.
pub fn same_moves(original: &str, translation: &str) -> bool {
    !translation.trim().is_empty() && moves_in(original) == moves_in(translation)
}

// --- The instructions ---------------------------------------------------------------------

/// English name of the target language for the model.
fn english_name(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "English",
        Lang::Ru => "Russian",
        Lang::Fr => "French",
        Lang::De => "German",
        Lang::Es => "Spanish",
        Lang::It => "Italian",
        Lang::Pt => "Brazilian Portuguese",
        Lang::Uz => "Uzbek (Latin script)",
        Lang::Kk => "Kazakh (Cyrillic script)",
    }
}

/// Chess terms as players of the language say them (English → target).
fn glossary(lang: Lang) -> &'static str {
    match lang {
        Lang::Ru => {
            "exchange (rook for a minor piece) — качество; to win the exchange — выиграть качество; trade, exchange of pieces — размен; passed pawn — проходная пешка; isolated pawn — изолированная пешка; doubled pawns — сдвоенные пешки; backward pawn — отсталая пешка; pawn chain — пешечная цепь; outpost — форпост; open file — открытая линия; half-open file — полуоткрытая линия; diagonal — диагональ; back rank — последняя горизонталь; fork — вилка; pin — связка; skewer — сквозной удар (рентген); discovered attack — вскрытое нападение; double attack — двойной удар; zwischenzug, intermediate move — промежуточный ход; sacrifice — жертва; combination — комбинация; initiative — инициатива; tempo — темп; zugzwang — цугцванг; stalemate — пат; perpetual check — вечный шах; opposition — оппозиция; fianchetto — фианкетто; bishop pair — два слона (преимущество двух слонов); minor piece — лёгкая фигура; major piece — тяжёлая фигура; development — развитие; castling — рокировка; endgame — эндшпиль; middlegame — миттельшпиль; opening — дебют; blunder — грубая ошибка (зевок); inaccuracy — неточность; time trouble — цейтнот; resigns — сдаётся; draw — ничья; decisive advantage — решающий перевес; compensation — компенсация"
        }
        Lang::De => {
            "exchange (rook for a minor piece) — Qualität; passed pawn — Freibauer; isolated pawn — isolierter Bauer (Isolani); doubled pawns — Doppelbauern; backward pawn — rückständiger Bauer; outpost — Vorposten; open file — offene Linie; back rank — Grundreihe; fork — Gabel; pin — Fesselung; skewer — Spieß; discovered attack — Abzugsangriff; zwischenzug — Zwischenzug; sacrifice — Opfer; initiative — Initiative; tempo — Tempo; zugzwang — Zugzwang; stalemate — Patt; perpetual check — Dauerschach; opposition — Opposition; fianchetto — Fianchetto; bishop pair — Läuferpaar; minor piece — Leichtfigur; major piece — Schwerfigur; development — Entwicklung; endgame — Endspiel; middlegame — Mittelspiel; opening — Eröffnung; blunder — grober Fehler (Patzer); time trouble — Zeitnot"
        }
        Lang::Fr => {
            "exchange (rook for a minor piece) — la qualité; passed pawn — pion passé; isolated pawn — pion isolé; doubled pawns — pions doublés; backward pawn — pion arriéré; outpost — avant-poste; open file — colonne ouverte; back rank — dernière rangée; fork — fourchette; pin — clouage; skewer — enfilade; discovered attack — attaque à la découverte; zwischenzug — coup intermédiaire; sacrifice — sacrifice; initiative — initiative; tempo — temps; zugzwang — zugzwang; stalemate — pat; perpetual check — échec perpétuel; opposition — opposition; fianchetto — fianchetto; bishop pair — paire de fous; minor piece — pièce mineure; major piece — pièce lourde; development — développement; endgame — finale; middlegame — milieu de partie; opening — ouverture; blunder — gaffe; time trouble — zeitnot (crise de temps)"
        }
        Lang::Es => {
            "exchange (rook for a minor piece) — la calidad; passed pawn — peón pasado; isolated pawn — peón aislado; doubled pawns — peones doblados; backward pawn — peón retrasado; outpost — puesto avanzado; open file — columna abierta; back rank — última fila; fork — horquilla (doble ataque); pin — clavada; skewer — ataque en rayos X (pincho); discovered attack — ataque a la descubierta; zwischenzug — jugada intermedia; sacrifice — sacrificio; initiative — iniciativa; tempo — tiempo; zugzwang — zugzwang; stalemate — ahogado; perpetual check — jaque perpetuo; opposition — oposición; fianchetto — fianchetto; bishop pair — pareja de alfiles; minor piece — pieza menor; major piece — pieza mayor; development — desarrollo; endgame — final; middlegame — medio juego; opening — apertura; blunder — error grave; time trouble — apuros de tiempo"
        }
        Lang::It => {
            "exchange (rook for a minor piece) — la qualità; passed pawn — pedone passato; isolated pawn — pedone isolato; doubled pawns — pedoni doppiati; backward pawn — pedone arretrato; outpost — avamposto; open file — colonna aperta; back rank — ultima traversa; fork — forchetta; pin — inchiodatura; skewer — infilata; discovered attack — attacco di scoperta; zwischenzug — mossa intermedia; sacrifice — sacrificio; initiative — iniziativa; tempo — tempo; zugzwang — zugzwang; stalemate — stallo; perpetual check — scacco perpetuo; opposition — opposizione; fianchetto — fianchetto; bishop pair — coppia degli alfieri; minor piece — pezzo leggero; major piece — pezzo pesante; development — sviluppo; endgame — finale; middlegame — mediogioco; opening — apertura; blunder — svista; time trouble — zeitnot"
        }
        Lang::Pt => {
            "exchange (rook for a minor piece) — a qualidade; passed pawn — peão passado; isolated pawn — peão isolado; doubled pawns — peões dobrados; backward pawn — peão atrasado; outpost — posto avançado; open file — coluna aberta; back rank — última fileira; fork — garfo; pin — cravada; skewer — espeto; discovered attack — ataque descoberto; zwischenzug — lance intermediário; sacrifice — sacrifício; initiative — iniciativa; tempo — tempo; zugzwang — zugzwang; stalemate — afogamento; perpetual check — xeque perpétuo; opposition — oposição; fianchetto — fianqueto; bishop pair — par de bispos; minor piece — peça menor; major piece — peça maior; development — desenvolvimento; endgame — final; middlegame — meio-jogo; opening — abertura; blunder — erro grave (capivarada); time trouble — apuro de tempo"
        }
        Lang::Uz => {
            "exchange (rook for a minor piece) — sifat; passed pawn — o‘tar piyoda; isolated pawn — yakkalangan piyoda; doubled pawns — qo‘shaloq piyodalar; open file — ochiq vertikal; fork — vilka; pin — bog‘lash; sacrifice — qurbonlik; initiative — tashabbus; tempo — temp; zugzwang — sugsvang; stalemate — pat; perpetual check — abadiy shoh; castling — roqirovka; endgame — endshpil; middlegame — mittelshpil; opening — debyut; blunder — qo‘pol xato; time trouble — vaqt tanqisligi (seytnot); draw — durang"
        }
        Lang::Kk => {
            "exchange (rook for a minor piece) — сапа; passed pawn — өтпелі сарбаз; isolated pawn — оқшау сарбаз; doubled pawns — қосарланған сарбаздар; open file — ашық тік сызық; fork — шанышқы; pin — байлау; sacrifice — құрбандық; initiative — бастама; tempo — темп; zugzwang — цугцванг; stalemate — пат; perpetual check — мәңгі шах; castling — рокировка; endgame — эндшпиль; middlegame — миттельшпиль; opening — дебют; blunder — өрескел қате; time trouble — уақыт тапшылығы (цейтнот); draw — тең ойын"
        }
        Lang::En => {
            "Russian качество — the exchange (rook for a minor piece); размен — exchange of pieces, trade; проходная — passed pawn; цугцванг — zugzwang; вилка — fork; связка — pin; вечный шах — perpetual check; цейтнот — time trouble; German Qualität — the exchange; Freibauer — passed pawn; Fesselung — pin; Gabel — fork; Abzug — discovered attack; Läuferpaar — bishop pair; Spanish/Italian calidad/qualità — the exchange; clavada/inchiodatura — pin; horquilla/forchetta — fork"
        }
    }
}

fn instructions(target: Lang) -> String {
    let lang = english_name(target);
    format!(
        "You translate the annotations (comments) of chess games into {lang} for chess players.\n\
         The input is JSON: {{\"game\": {{…}}, \"comments\": [{{\"id\": number, \"text\": string}}]}}. \
         \"game\" names the players, the event and the opening: use it to get names and terms right; do not translate it.\n\
         Rules:\n\
         1. Translate only the natural-language text of each comment into {lang}.\n\
         2. Copy every chess move and square exactly as written, character for character: e4, Nf3, Bxe5+, exd8=Q#, O-O, O-O-O, 12.Nf3, 12...Nf6, the d-file, the e5-square. Never translate, transliterate or replace the piece letters K, Q, R, B, N inside moves, and never add or remove moves.\n\
         3. Keep the names of people, events and places as written; keep numbers, results (1-0, ½-½), evaluation symbols (+-, =, ±, ∓, !, ?, !?, ?!) and URLs. Text in brackets is text: translate it too.\n\
         4. A part that is already in {lang} stays as it is.\n\
         5. Use the chess terms {lang}-speaking players use. Glossary: {glossary}.\n\
         6. Do not add, drop, explain, summarize or comment on anything. Keep the tone, the paragraph breaks and the meaning.\n\
         7. Answer with JSON only, no other text: {{\"translations\": [{{\"id\": number, \"text\": string}}]}} with every id of the input exactly once.",
        glossary = glossary(target)
    )
}

/// The players, the event and the opening, for the model.
#[derive(Clone, Debug, Default, Serialize)]
pub struct GameContext {
    pub white: String,
    pub black: String,
    pub event: String,
    pub opening: String,
}

impl GameContext {
    pub fn of(game: &Game) -> GameContext {
        let tag = |t| game.tag(t).unwrap_or("").to_owned();
        let opening = [tag("Opening"), tag("Variation")]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        GameContext {
            white: tag("White"),
            black: tag("Black"),
            event: tag("Event"),
            opening: if opening.is_empty() {
                tag("ECO")
            } else {
                opening
            },
        }
    }
}

fn request(context: &GameContext, batch: &[(usize, &str)]) -> String {
    let comments: Vec<Value> = batch
        .iter()
        .map(|(id, text)| json!({"id": id, "text": text}))
        .collect();
    json!({"game": context, "comments": comments}).to_string()
}

/// The translations in an answer, by id. Models sometimes wrap JSON in prose or a code
/// fence, give ids as strings, or a map instead of a list: the first object with
/// translations is taken.
fn parse_answer(answer: &str) -> HashMap<usize, String> {
    let mut out = HashMap::new();
    let id = |v: &Value| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .map(|n| n as usize)
    };
    for (start, _) in answer.match_indices('{') {
        let Some(Ok(v)) = serde_json::Deserializer::from_str(&answer[start..])
            .into_iter::<Value>()
            .next()
        else {
            continue;
        };
        let Some(list) = v.get("translations").or_else(|| v.get("comments")) else {
            continue;
        };
        if let Some(items) = list.as_array() {
            for item in items {
                if let (Some(n), Some(text)) = (
                    item.get("id").and_then(id),
                    item.get("text").and_then(Value::as_str),
                ) {
                    out.insert(n, text.trim().to_owned());
                }
            }
        } else if let Some(map) = list.as_object() {
            for (k, t) in map {
                if let (Ok(n), Some(text)) = (k.trim().parse::<usize>(), t.as_str()) {
                    out.insert(n, text.trim().to_owned());
                }
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

// --- Errors ------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The key (or the sign-in) is not accepted.
    Key,
    /// The account has no money left (DeepSeek: 402).
    Money,
    /// Too many requests even after waiting.
    RateLimited,
    /// The ChatGPT limit for ChessPenguin is used up.
    PlanLimit,
    /// The ChatGPT account cannot use its plan here (a free plan, a region).
    NotEligible(String),
    /// Signed out of ChatGPT (revoked, expired): sign in again.
    SignedOut,
    /// No connection to the service.
    Offline(String),
    /// The model on this computer does not answer (Ollama, LM Studio not running).
    LocalDown,
    /// The service has no such model.
    NoModel(String),
    /// Another error from the service: status and what it said.
    Service(u16, String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Key => write!(f, "the key is not accepted"),
            Failure::Money => write!(f, "insufficient balance"),
            Failure::RateLimited => write!(f, "rate limited"),
            Failure::PlanLimit => write!(f, "subscription_sharing_usage_limit_exceeded"),
            Failure::NotEligible(e) => write!(f, "not eligible: {e}"),
            Failure::SignedOut => write!(f, "signed out"),
            Failure::Offline(e) => write!(f, "network: {e}"),
            Failure::LocalDown => write!(f, "the local model does not answer"),
            Failure::NoModel(m) => write!(f, "no model {m}"),
            Failure::Service(code, body) => write!(f, "HTTP {code}: {body}"),
        }
    }
}

pub fn agent(body_timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .user_agent(format!(
            "ChessPenguin/{} (+https://chess.vrubel.xyz)",
            env!("CARGO_PKG_VERSION")
        ))
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(10)))
        // A model on a slow computer may think for minutes.
        .timeout_recv_response(Some(body_timeout))
        .timeout_recv_body(Some(body_timeout))
        .build();
    ureq::Agent::new_with_config(config)
}

/// What a service said in an error answer (OpenAI style `error.message`, or `detail`).
fn error_text(body: &str) -> String {
    let v: Option<Value> = serde_json::from_str(body).ok();
    let msg = v.as_ref().and_then(|v| {
        v.pointer("/error/message")
            .or_else(|| v.get("error"))
            .or_else(|| v.get("detail"))
            .and_then(|m| m.as_str().map(str::to_owned))
    });
    let text = msg.unwrap_or_else(|| body.trim().to_owned());
    text.chars().take(300).collect()
}

// --- Backends ----------------------------------------------------------------------------

/// Where one translation job sends its requests.
#[derive(Clone, Debug)]
pub enum Backend {
    Chat {
        base: String,
        key: Option<String>,
        model: String,
        json_mode: bool,
        local: bool,
    },
    ChatGpt {
        model: String,
    },
}

impl Backend {
    /// From the settings (`None`: not set up yet).
    pub fn from(s: &TranslationSettings) -> Option<Backend> {
        if !s.ready() {
            return None;
        }
        Some(match s.connection {
            Connection::ChatGpt => Backend::ChatGpt {
                model: s.chatgpt_model.clone(),
            },
            Connection::Key => Backend::Chat {
                base: s.base(),
                key: credentials().key(s.preset.id()).map(str::to_owned),
                model: s.model(),
                json_mode: s.preset.json_mode(),
                local: s.is_local(),
            },
        })
    }

    /// The model's answer to one request.
    pub fn complete(
        &self,
        system: &str,
        user: &str,
        cancel: &AtomicBool,
    ) -> Result<String, Failure> {
        match self {
            Backend::Chat {
                base,
                key,
                model,
                json_mode,
                local,
            } => chat(
                base,
                key.as_deref(),
                model,
                *json_mode,
                *local,
                system,
                user,
                cancel,
            ),
            Backend::ChatGpt { model } => crate::chatgpt::respond(model, system, user, cancel),
        }
    }
}

/// The models a key connection offers (`GET {base}/models`).
pub fn list_models(base: &str, key: Option<&str>, local: bool) -> Result<Vec<String>, Failure> {
    let mut req = agent(Duration::from_secs(30)).get(format!("{base}/models"));
    if let Some(k) = key {
        req = req.header("Authorization", format!("Bearer {k}"));
    }
    let mut resp = req.call().map_err(|e| net_failure(e, local))?;
    let status = resp.status().as_u16();
    let body = resp
        .body_mut()
        .with_config()
        .limit(16 << 20)
        .read_to_string()
        .unwrap_or_default();
    if status != 200 {
        return Err(status_failure(status, &body, ""));
    }
    let v: Value =
        serde_json::from_str(&body).map_err(|e| Failure::Service(status, e.to_string()))?;
    let mut ids: Vec<String> = v
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        // Gemini lists "models/gemini-…".
        .map(|id| id.trim_start_matches("models/").to_owned())
        .collect();
    ids.sort();
    Ok(ids)
}

fn net_failure(e: ureq::Error, local: bool) -> Failure {
    if local {
        Failure::LocalDown
    } else {
        Failure::Offline(e.to_string())
    }
}

fn status_failure(status: u16, body: &str, model: &str) -> Failure {
    let text = error_text(body);
    let lower = text.to_lowercase();
    match status {
        401 | 403 => Failure::Key,
        402 => Failure::Money,
        429 if lower.contains("quota") || lower.contains("balance") => Failure::Money,
        429 => Failure::RateLimited,
        404 if lower.contains("model") => Failure::NoModel(model.to_owned()),
        400 if lower.contains("model") && lower.contains("not") => {
            Failure::NoModel(model.to_owned())
        }
        _ => Failure::Service(status, text),
    }
}

#[allow(clippy::too_many_arguments)]
fn chat(
    base: &str,
    key: Option<&str>,
    model: &str,
    json_mode: bool,
    local: bool,
    system: &str,
    user: &str,
    cancel: &AtomicBool,
) -> Result<String, Failure> {
    let mut body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "temperature": 0.2,
        "stream": false,
    });
    if json_mode {
        body["response_format"] = json!({"type": "json_object"});
    }
    let text = body.to_string();
    with_waits(cancel, || {
        let mut req = agent(Duration::from_secs(if local { 300 } else { 180 }))
            .post(format!("{base}/chat/completions"))
            .header("Content-Type", "application/json");
        if let Some(k) = key {
            req = req.header("Authorization", format!("Bearer {k}"));
        }
        let mut resp = req.send(&text).map_err(|e| net_failure(e, local))?;
        let status = resp.status().as_u16();
        let answer = resp
            .body_mut()
            .with_config()
            .limit(16 << 20)
            .read_to_string()
            .map_err(|e| net_failure(e, local))?;
        if status != 200 {
            return Err(status_failure(status, &answer, model));
        }
        let v: Value =
            serde_json::from_str(&answer).map_err(|e| Failure::Service(status, e.to_string()))?;
        v.pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Failure::Service(status, error_text(&answer)))
    })
}

/// Seconds to wait before the next attempt after "too many requests" or a passing failure.
const WAITS: [u64; 3] = [5, 20, 40];

/// Status of a waiting job (seconds left), read by the interface.
pub static WAITING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Repeats a request after a rate limit or a server hiccup, with waits; gives up on the
/// others at once.
pub fn with_waits<T>(
    cancel: &AtomicBool,
    mut request: impl FnMut() -> Result<T, Failure>,
) -> Result<T, Failure> {
    let mut waits = WAITS.iter();
    loop {
        match request() {
            Err(e @ (Failure::RateLimited | Failure::Service(500..=599, _))) => {
                let Some(&secs) = waits.next() else {
                    return Err(e);
                };
                for left in (1..=secs * 10).rev() {
                    if cancel.load(Ordering::Relaxed) {
                        WAITING.store(0, Ordering::Relaxed);
                        return Err(e);
                    }
                    WAITING.store(left.div_ceil(10), Ordering::Relaxed);
                    std::thread::sleep(Duration::from_millis(100));
                }
                WAITING.store(0, Ordering::Relaxed);
            }
            other => return other,
        }
    }
}

/// A fixed sentence translated to check a connection (not the user's text).
pub fn sample(target: Lang) -> &'static str {
    if target == Lang::En {
        "Der Springer ist auf d5 stark."
    } else {
        "The knight is strong on d5."
    }
}

/// Translates the check sentence: the connection works when it comes back with its move.
pub fn check(backend: &Backend, target: Lang, cancel: &AtomicBool) -> Result<String, Failure> {
    let text = sample(target);
    let answer = backend.complete(
        &instructions(target),
        &request(&GameContext::default(), &[(0, text)]),
        cancel,
    )?;
    parse_answer(&answer)
        .remove(&0)
        .filter(|t| same_moves(text, t))
        .ok_or_else(|| Failure::Service(200, answer.chars().take(300).collect()))
}

// --- The cache ---------------------------------------------------------------------------

/// Translations by target language, then by the hash of the original text.
static CACHE: Mutex<Option<HashMap<Lang, HashMap<String, String>>>> = Mutex::new(None);

fn cache_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return Some(std::env::temp_dir().join(format!("cp-translations-{}", std::process::id())));
    }
    crate::settings::Settings::cache_dir().map(|d| d.join("translations"))
}

fn lang_code(l: Lang) -> &'static str {
    match l {
        Lang::En => "en",
        Lang::Ru => "ru",
        Lang::Fr => "fr",
        Lang::De => "de",
        Lang::Es => "es",
        Lang::It => "it",
        Lang::Pt => "pt",
        Lang::Uz => "uz",
        Lang::Kk => "kk",
    }
}

/// The format version is part of the key: new instructions give new translations.
fn key(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"v2\0");
    h.update(text.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Serialize, Deserialize)]
struct CacheLine {
    k: String,
    t: String,
    #[serde(default)]
    via: String,
}

fn with_cache<R>(target: Lang, f: impl FnOnce(&mut HashMap<String, String>) -> R) -> R {
    let mut all = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let all = all.get_or_insert_with(HashMap::new);
    let map = all.entry(target).or_insert_with(|| {
        let mut m = HashMap::new();
        if let Some(f) = cache_dir().map(|d| d.join(format!("{}.jsonl", lang_code(target))))
            && let Ok(file) = std::fs::File::open(f)
        {
            for line in BufReader::new(file).lines().map_while(Result::ok) {
                if let Ok(l) = serde_json::from_str::<CacheLine>(&line) {
                    m.insert(l.k, l.t);
                }
            }
        }
        m
    });
    f(map)
}

pub fn cached(target: Lang, text: &str) -> Option<String> {
    with_cache(target, |m| m.get(&key(text)).cloned())
}

fn store(target: Lang, pairs: &[(String, String)], via: &str) {
    with_cache(target, |m| {
        let Some(dir) = cache_dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{}.jsonl", lang_code(target))))
        else {
            return;
        };
        for (orig, t) in pairs {
            let k = key(orig);
            if let Ok(line) = serde_json::to_string(&CacheLine {
                k: k.clone(),
                t: t.clone(),
                via: via.to_owned(),
            }) {
                let _ = writeln!(file, "{line}");
            }
            m.insert(k, t.clone());
        }
    });
}

/// Bytes the cache takes on the disk.
pub fn cache_size() -> u64 {
    let Some(dir) = cache_dir() else { return 0 };
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

pub fn clear_cache() {
    if let Some(dir) = cache_dir() {
        let _ = std::fs::remove_dir_all(dir);
    }
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// --- A translation job ---------------------------------------------------------------------

pub enum Event {
    /// Translated: (original, translation).
    Done(Vec<(String, String)>),
    /// These could not be translated (the answer lost or changed moves, or was empty).
    Bad(Vec<String>),
    /// The job stopped on this error; what was done is kept.
    Failed(Failure),
    Finished,
}

pub struct Job {
    rx: Receiver<Event>,
    cancel: Arc<AtomicBool>,
}

/// About this much text goes in one request.
const BATCH_CHARS: usize = 5000;
const BATCH_ITEMS: usize = 40;

/// These texts go in one request (then a count "n of total" says nothing).
pub fn one_batch(texts: &[String]) -> bool {
    texts.len() <= BATCH_ITEMS && texts.iter().map(|t| t.len()).sum::<usize>() <= BATCH_CHARS
}

impl Job {
    pub fn start(
        ctx: &egui::Context,
        backend: Backend,
        target: Lang,
        via: String,
        context: GameContext,
        texts: Vec<String>,
    ) -> Job {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("translate".into())
            .spawn(move || {
                run(&backend, target, &via, &context, texts, &c, &tx, &ctx);
                let _ = tx.send(Event::Finished);
                ctx.request_repaint();
            });
        Job { rx, cancel }
    }

    pub fn poll(&self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.stop();
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    backend: &Backend,
    target: Lang,
    via: &str,
    context: &GameContext,
    texts: Vec<String>,
    cancel: &AtomicBool,
    tx: &Sender<Event>,
    ctx: &egui::Context,
) {
    let system = instructions(target);
    // No model chosen yet (a model on this computer never checked): the first listed.
    let mut backend = backend.clone();
    if let Backend::Chat {
        base,
        key,
        model,
        local,
        ..
    } = &mut backend
        && model.is_empty()
    {
        match list_models(base, key.as_deref(), *local) {
            Ok(m) if !m.is_empty() => *model = m[0].clone(),
            Ok(_) => {
                let _ = tx.send(Event::Failed(Failure::NoModel(String::new())));
                return;
            }
            Err(e) => {
                let _ = tx.send(Event::Failed(e));
                return;
            }
        }
    }
    // ChatGPT without a model yet (the check after the sign-in did not get that far).
    if let Backend::ChatGpt { model } = &mut backend
        && model.is_empty()
    {
        match crate::chatgpt::models() {
            Ok(m) => match crate::chatgpt::default_model(&m) {
                Some(d) => *model = d,
                None => {
                    let _ = tx.send(Event::Failed(Failure::NoModel(String::new())));
                    return;
                }
            },
            Err(e) => {
                let _ = tx.send(Event::Failed(e));
                return;
            }
        }
    }
    let backend = &backend;
    let mut batches: Vec<Vec<(usize, &str)>> = Vec::new();
    let mut chars = 0;
    for (i, t) in texts.iter().enumerate() {
        let last = batches.last().map_or(0, Vec::len);
        if batches.is_empty() || chars + t.len() > BATCH_CHARS || last >= BATCH_ITEMS {
            batches.push(Vec::new());
            chars = 0;
        }
        chars += t.len();
        if let Some(b) = batches.last_mut() {
            b.push((i, t.as_str()));
        }
    }
    let send = |e: Event| {
        let _ = tx.send(e);
        ctx.request_repaint();
    };
    for batch in batches {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let answer = match backend.complete(&system, &request(context, &batch), cancel) {
            Ok(a) => a,
            Err(e) => {
                if !cancel.load(Ordering::Relaxed) {
                    send(Event::Failed(e));
                }
                return;
            }
        };
        let got = parse_answer(&answer);
        let mut done = Vec::new();
        let mut again = Vec::new();
        for &(id, text) in &batch {
            match got.get(&id) {
                Some(t) if same_moves(text, t) => done.push((text.to_owned(), t.clone())),
                _ => again.push((id, text)),
            }
        }
        // One more try for each comment that came back wrong, alone.
        let mut bad = Vec::new();
        for (id, text) in again {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let retry = backend
                .complete(&system, &request(context, &[(id, text)]), cancel)
                .map(|a| parse_answer(&a));
            match retry {
                Ok(got) => match got.get(&id) {
                    Some(t) if same_moves(text, t) => done.push((text.to_owned(), t.clone())),
                    _ => bad.push(text.to_owned()),
                },
                Err(e) => {
                    store(target, &done, via);
                    send(Event::Done(done));
                    if !cancel.load(Ordering::Relaxed) {
                        send(Event::Failed(e));
                    }
                    return;
                }
            }
        }
        store(target, &done, via);
        send(Event::Done(done));
        if !bad.is_empty() {
            send(Event::Bad(bad));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "prints the instructions for a manual check"]
    fn print_instructions() {
        // TRANSLATE_LANG=en|ru|fr|de|es|it (Russian by default).
        let lang = match std::env::var("TRANSLATE_LANG").as_deref() {
            Ok("en") => Lang::En,
            Ok("fr") => Lang::Fr,
            Ok("de") => Lang::De,
            Ok("es") => Lang::Es,
            Ok("it") => Lang::It,
            _ => Lang::Ru,
        };
        println!("{}", instructions(lang));
    }

    #[test]
    fn moves_are_found_in_text() {
        let t = "After 12.Nf3 Bxe5+ 13.dxe5 (13.O-O!?) the e5-square is weak; 14...Kh8.";
        assert_eq!(
            moves_in(t),
            vec!["Bxe5", "Kh8", "Nf3", "O-O", "dxe5", "e5"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        let one = |w: &str| moves_in(w);
        assert_eq!(one("exd8=Q#"), vec!["exd8=Q"]);
        assert_eq!(one("Nbd7"), vec!["Nbd7"]);
        assert_eq!(one("R1e2"), vec!["R1e2"]);
        assert_eq!(one("Nf3xe5"), vec!["Nf3xe5"]);
        assert_eq!(one("e4"), vec!["e4"]);
        assert_eq!(one("0-0-0"), vec!["O-O-O"]);
        assert_eq!(one("e8Q"), vec!["e8=Q"]);
        assert_eq!(one("a4-a5"), vec!["a4", "a5"]);
        assert_eq!(one("Nf3/Ng5"), vec!["Nf3", "Ng5"]);
        assert_eq!(one("the e5-square"), vec!["e5"]);
        assert_eq!(one("13.Qd2,Rd1"), vec!["Qd2", "Rd1"]);
        assert_eq!(one("♘f3"), vec!["♘f3"]);
        assert_eq!(one("Крg1 Фd8"), vec!["Крg1", "Фd8"]);
        for word in ["Be", "the", "1-0", "12.", "a", "h9", "Zf3", "lead4", "Bb"] {
            assert!(one(word).is_empty(), "{word}: {:?}", one(word));
        }
    }

    #[test]
    fn a_translation_must_keep_the_moves() {
        let orig = "White should play 12.Nf3, not 12.Bd3?";
        assert!(same_moves(
            orig,
            "Белым следовало играть 12.Nf3, а не 12.Bd3?"
        ));
        assert!(!same_moves(
            orig,
            "Белым следовало играть 12.Кf3, а не 12.Bd3?"
        ));
        assert!(!same_moves(orig, "Белым следовало играть 12.Nf3."));
        assert!(!same_moves(orig, " "));
        assert!(same_moves("The e5-square is weak.", "Поле e5 слабое."));
        assert!(same_moves("13.Qd2,Rd1", "13.Qd2, Rd1"));
        assert!(same_moves("Nf3— strong", "Nf3 — сильно"));
        assert!(!same_moves("Nf3/Ng5", "Nf3/Nh5"));
        assert!(!same_moves("a4-a5", "a4-a6"));
        assert!(!same_moves("♘f3!", "Кf3!"));
    }

    #[test]
    fn foreign_comments_are_offered_and_short_ones_follow_the_game() {
        let texts: Vec<String> = [
            "White has a clear advantage because the knight on d5 dominates the board.",
            "Weak.",
            "Black should have played for the initiative on the queenside instead.",
            "A typical mistake: the pawn structure is ruined and the endgame is lost.",
            "+-",
        ]
        .map(String::from)
        .to_vec();
        let p = plan(&texts, Lang::Ru).expect("English comments for a Russian reader");
        assert_eq!(p.source, whatlang::Lang::Eng);
        assert_eq!(p.texts.len(), 4, "{:?}", p.texts);
        assert!(p.texts.contains(&"Weak.".to_string()));
        assert!(!p.texts.contains(&"+-".to_string()));
        assert_eq!(plan(&texts, Lang::En), None);
    }

    #[test]
    fn a_game_of_short_comments_is_still_understood() {
        let texts: Vec<String> = [
            "Weak move.",
            "Better was this.",
            "The only move.",
            "A blunder now.",
            "Good idea here.",
            "White wins.",
            "Black is lost.",
            "Too slow.",
            "Much stronger.",
            "A draw then.",
        ]
        .map(String::from)
        .to_vec();
        let p = plan(&texts, Lang::Ru).expect("short English comments");
        assert_eq!(p.source, whatlang::Lang::Eng);
        assert_eq!(p.texts.len(), 10);
    }

    #[test]
    fn one_foreign_word_is_not_offered() {
        let texts = vec!["Zugzwang!".to_string(), "1-0".to_string()];
        assert_eq!(plan(&texts, Lang::Ru), None);
    }

    #[test]
    fn russian_comments_stay_for_a_russian_reader() {
        let texts: Vec<String> = [
            "Белые получают явный перевес, конь на d5 господствует на доске.",
            "Слабо.",
            "Черным следовало искать контригру на ферзевом фланге.",
            "Типичная ошибка: пешечная структура испорчена, эндшпиль проигран.",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(plan(&texts, Lang::Ru), None);
        assert_eq!(plan(&texts, Lang::En).map(|p| p.texts.len()), Some(4));
    }

    #[test]
    fn answers_are_read_also_from_prose_and_fences() {
        let a = "Sure!\n```json\n{\"translations\": [{\"id\": 0, \"text\": \" Конь силён \"}, {\"id\": 2, \"text\": \"Слабо\"}]}\n```";
        let got = parse_answer(a);
        assert_eq!(got.get(&0).map(String::as_str), Some("Конь силён"));
        assert_eq!(got.get(&2).map(String::as_str), Some("Слабо"));
        assert!(parse_answer("no json here").is_empty());
        let a = r#"Here {note}: {"translations": [{"id": "1", "text": "Да"}]} {:)}"#;
        assert_eq!(parse_answer(a).get(&1).map(String::as_str), Some("Да"));
        let a = r#"{"translations": {"0": "Конь", "3": "Слон"}}"#;
        let got = parse_answer(a);
        assert_eq!(got.get(&3).map(String::as_str), Some("Слон"));
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn local_addresses() {
        assert!(is_local_address("http://127.0.0.1:11434/v1"));
        assert!(is_local_address("http://localhost:1234/v1"));
        assert!(!is_local_address("https://api.deepseek.com"));
        assert!(!is_local_address("http://192.168.1.5:11434/v1"));
    }

    #[test]
    fn batches_name_each_comment() {
        let r = request(&GameContext::default(), &[(0, "a"), (1, "b")]);
        let v: Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["comments"][1]["id"], 1);
        assert_eq!(v["comments"][1]["text"], "b");
    }
    /// An OpenAI-compatible server on 127.0.0.1 that "translates" by prefixing each
    /// comment, and breaks the moves of comments containing "BREAK".
    fn fake_service(requests: usize) -> (String, std::thread::JoinHandle<Vec<Value>>) {
        use std::io::Read as _;
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let base = format!(
            "http://127.0.0.1:{}/v1",
            listener.local_addr().unwrap().port()
        );
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..requests {
                let (mut s, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(s.try_clone().unwrap());
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                let req: Value = serde_json::from_slice(&body).unwrap();
                let user: Value =
                    serde_json::from_str(req["messages"][1]["content"].as_str().unwrap()).unwrap();
                let out: Vec<Value> = user["comments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| {
                        let t = c["text"].as_str().unwrap();
                        let t = if t.contains("BREAK") {
                            t.replace("Nf3", "Кf3")
                        } else {
                            t.to_owned()
                        };
                        json!({"id": c["id"], "text": format!("RU {t}")})
                    })
                    .collect();
                let content = json!({"translations": out}).to_string();
                let answer = json!({"choices": [{"message": {"content": content}}]}).to_string();
                write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                )
                .unwrap();
                seen.push(req);
            }
            seen
        });
        (base, server)
    }

    #[test]
    fn a_job_translates_checks_moves_and_caches() {
        // One batch, then the broken comment alone once more.
        let (base, server) = fake_service(2);
        let backend = Backend::Chat {
            base,
            key: None,
            model: "m".into(),
            json_mode: false,
            local: true,
        };
        let texts = vec![
            "After 12.Nf3 White is better.".to_string(),
            "BREAK after 12.Nf3 the game is over.".to_string(),
        ];
        let job = Job::start(
            &egui::Context::default(),
            backend,
            Lang::It,
            "test".into(),
            GameContext::default(),
            texts.clone(),
        );
        let mut done = Vec::new();
        let mut bad = Vec::new();
        let start = std::time::Instant::now();
        'wait: while start.elapsed() < Duration::from_secs(20) {
            for e in job.poll() {
                match e {
                    Event::Done(d) => done.extend(d),
                    Event::Bad(b) => bad.extend(b),
                    Event::Failed(f) => panic!("{f}"),
                    Event::Finished => break 'wait,
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(done, vec![(texts[0].clone(), format!("RU {}", texts[0]))]);
        assert_eq!(bad, vec![texts[1].clone()]);
        assert_eq!(
            cached(Lang::It, &texts[0]),
            Some(format!("RU {}", texts[0]))
        );
        assert_eq!(cached(Lang::It, &texts[1]), None);
        let seen = server.join().unwrap();
        assert!(
            seen[0]["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("Italian")
        );
        assert!(seen[0].get("response_format").is_none());
    }
}
