//! The game's page on the site it was played on: Lichess writes it in the Site tag
//! («https://lichess.org/AbCdEfGh»), Chess.com in the Link tag (its Site is
//! «Chess.com»). A Lichess game opens at the move shown and from the side at the
//! bottom of the board; Chess.com's page has no such address, it opens at the start.

use crate::i18n::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    Lichess,
    ChessCom,
    /// Another web address.
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameLink {
    pub url: String,
    pub site: Site,
    /// The Lichess game id (8 characters), when the address is a game's.
    lichess_id: Option<String>,
}

/// The game's web address from its tags (Link first, then Site), if it has one.
pub fn of(tag: impl Fn(&str) -> Option<String>) -> Option<GameLink> {
    let url = ["Link", "Site"]
        .into_iter()
        .filter_map(&tag)
        .map(|v| v.trim().to_owned())
        .find(|v| v.starts_with("https://") || v.starts_with("http://"))?;
    let host_and_path = url.split_once("://").map_or("", |(_, rest)| rest);
    let (host, path) = host_and_path.split_once('/').unwrap_or((host_and_path, ""));
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let site = match host {
        "lichess.org" => Site::Lichess,
        "chess.com" => Site::ChessCom,
        _ => Site::Other,
    };
    // «AbCdEfGh», or the 12-character id of one player's view of it.
    let first = path.split(['/', '#', '?']).next().unwrap_or("");
    let lichess_id = (site == Site::Lichess
        && (first.len() == 8 || first.len() == 12)
        && first.chars().all(|c| c.is_ascii_alphanumeric()))
    .then(|| first[..8].to_owned());
    Some(GameLink {
        url,
        site,
        lichess_id,
    })
}

impl GameLink {
    /// The address to open: a Lichess game at half-move `ply` (0: the start) from
    /// Black's side when `black`; any other as written.
    pub fn at(&self, ply: u32, black: bool) -> String {
        match &self.lichess_id {
            Some(id) => {
                let mut url = format!("https://lichess.org/{id}");
                if black {
                    url.push_str("/black");
                }
                if ply > 0 {
                    url.push_str(&format!("#{ply}"));
                }
                url
            }
            None => self.url.clone(),
        }
    }

    /// «🌐 Open on Lichess».
    pub fn label(&self) -> &'static str {
        match self.site {
            Site::Lichess => tr("🌐 Open on Lichess"),
            Site::ChessCom => tr("🌐 Open on Chess.com"),
            Site::Other => tr("🌐 Open the game's link"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(tags: &[(&str, &str)]) -> Option<GameLink> {
        of(|name| {
            tags.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| (*v).to_owned())
        })
    }

    #[test]
    fn lichess_chess_com_and_others() {
        let l = link(&[("Site", "https://lichess.org/AbCdEfGh")]).expect("lichess");
        assert_eq!(l.site, Site::Lichess);
        assert_eq!(l.at(0, false), "https://lichess.org/AbCdEfGh");
        assert_eq!(l.at(23, true), "https://lichess.org/AbCdEfGh/black#23");
        // One player's view (12 characters): the game.
        let l = link(&[("Site", "https://lichess.org/AbCdEfGh1234")]).unwrap();
        assert_eq!(l.at(4, false), "https://lichess.org/AbCdEfGh#4");
        // A study is opened as it is.
        let s = link(&[("Site", "https://lichess.org/study/abcdefgh/ijklmnop")]).unwrap();
        assert_eq!(
            s.at(10, true),
            "https://lichess.org/study/abcdefgh/ijklmnop"
        );
        let c = link(&[
            ("Site", "Chess.com"),
            ("Link", "https://www.chess.com/game/live/123456789"),
        ])
        .expect("chess.com");
        assert_eq!(c.site, Site::ChessCom);
        assert_eq!(c.at(12, true), "https://www.chess.com/game/live/123456789");
        assert_eq!(
            link(&[("Site", "http://example.org/g/1")]).unwrap().site,
            Site::Other
        );
        assert!(link(&[("Site", "Moscow RUS")]).is_none());
        assert!(link(&[("Site", "Chess.com")]).is_none());
    }
}
