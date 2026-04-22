use serde_json::Value;
use std::str;

use crate::error::{Result, RoflError};

/// Side of the map a player was on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Team {
    Blue,
    Red,
}

/// Lane / role assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Top,
    Jungle,
    Mid,
    Adc,
    Support,
}

/// A per-match player record, as extracted from `statsJson`.
#[derive(Debug, Clone)]
pub struct Player {
    /// Riot ID game-name portion, if present. `None` if the `statsJson`
    /// entry didn't have the key (older replays).
    pub riot_id_game_name: Option<String>,
    /// Stable player identifier.
    pub puuid: Option<String>,
    /// Champion name as stored, e.g. `"Sett"`.
    pub skin: String,
    pub team: Team,
    /// Role, preferring `INDIVIDUAL_POSITION` when present; falling back to
    /// the canonical role-by-index order Mowokuma uses.
    pub role: Role,
    pub win: bool,
}

/// Trailing metadata of a ROFL file.
#[derive(Debug, Clone)]
pub struct Metadata {
    pub game_length_ms: u64,
    pub last_game_chunk_id: u64,
    pub last_key_frame_id: u64,
    /// The raw `statsJson` string (JSON-in-JSON), preserved for round-trip.
    pub stats_json_raw: String,
    pub players: Vec<Player>,
    pub winning_team: Team,
}

impl Metadata {
    /// Parse the metadata-JSON bytes (the region between the signature and
    /// the trailing u32 length).
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let text = str::from_utf8(bytes).map_err(|_| RoflError::MetadataNotUtf8)?;
        let v: Value = serde_json::from_str(text)?;

        let game_length_ms = v
            .get("gameLength")
            .and_then(Value::as_i64)
            .ok_or(RoflError::MetadataMissingKey("gameLength"))? as u64;

        let last_game_chunk_id = v
            .get("lastGameChunkId")
            .and_then(Value::as_i64)
            .ok_or(RoflError::MetadataMissingKey("lastGameChunkId"))? as u64;

        let last_key_frame_id = v
            .get("lastKeyFrameId")
            .and_then(Value::as_i64)
            .ok_or(RoflError::MetadataMissingKey("lastKeyFrameId"))? as u64;

        let stats_json_raw = v
            .get("statsJson")
            .and_then(Value::as_str)
            .ok_or(RoflError::MetadataMissingKey("statsJson"))?
            .to_string();

        let stats_json: Value = serde_json::from_str(&stats_json_raw)?;
        let stats_arr = stats_json
            .as_array()
            .ok_or(RoflError::MetadataMissingKey("statsJson[]"))?;

        let mut players: Vec<Player> = Vec::with_capacity(stats_arr.len());
        for (i, p) in stats_arr.iter().enumerate() {
            let obj = p
                .as_object()
                .ok_or(RoflError::MetadataMissingKey("statsJson[i]"))?;

            let skin = obj
                .get("SKIN")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();

            let team = match obj.get("TEAM").and_then(Value::as_str) {
                Some("100") => Team::Blue,
                Some("200") => Team::Red,
                _ => Team::Blue,
            };

            let win = obj.get("WIN").and_then(Value::as_str) == Some("Win");

            let riot_id_game_name = obj
                .get("RIOT_ID_GAME_NAME")
                .and_then(Value::as_str)
                .map(str::to_string);
            let puuid = obj
                .get("PUUID")
                .and_then(Value::as_str)
                .map(str::to_string);

            let role = obj
                .get("INDIVIDUAL_POSITION")
                .and_then(Value::as_str)
                .and_then(parse_role)
                .unwrap_or_else(|| role_from_index(i));

            players.push(Player {
                riot_id_game_name,
                puuid,
                skin,
                team,
                role,
                win,
            });
        }

        let winning_team = match players.first() {
            Some(p) if p.win => p.team,
            Some(p) => match p.team {
                Team::Blue => Team::Red,
                Team::Red => Team::Blue,
            },
            None => Team::Blue,
        };

        Ok(Metadata {
            game_length_ms,
            last_game_chunk_id,
            last_key_frame_id,
            stats_json_raw,
            players,
            winning_team,
        })
    }
}

fn role_from_index(i: usize) -> Role {
    match i % 5 {
        0 => Role::Top,
        1 => Role::Jungle,
        2 => Role::Mid,
        3 => Role::Adc,
        4 => Role::Support,
        _ => unreachable!(),
    }
}

fn parse_role(s: &str) -> Option<Role> {
    match s.to_uppercase().as_str() {
        "TOP" => Some(Role::Top),
        "JUNGLE" => Some(Role::Jungle),
        "MIDDLE" | "MID" => Some(Role::Mid),
        "BOTTOM" | "ADC" | "BOT" => Some(Role::Adc),
        "UTILITY" | "SUPPORT" => Some(Role::Support),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_synthetic_metadata() {
        let stats = serde_json::json!([
            {"SKIN": "Poppy", "TEAM": "100", "WIN": "Win"},
            {"SKIN": "MasterYi", "TEAM": "100", "WIN": "Win"},
            {"SKIN": "Azir", "TEAM": "100", "WIN": "Win"},
            {"SKIN": "Ezreal", "TEAM": "100", "WIN": "Win"},
            {"SKIN": "Maokai", "TEAM": "100", "WIN": "Win"},
            {"SKIN": "Shen", "TEAM": "200", "WIN": "Fail"},
            {"SKIN": "Sejuani", "TEAM": "200", "WIN": "Fail"},
            {"SKIN": "Katarina", "TEAM": "200", "WIN": "Fail"},
            {"SKIN": "MissFortune", "TEAM": "200", "WIN": "Fail"},
            {"SKIN": "Nautilus", "TEAM": "200", "WIN": "Fail"},
        ]);
        let outer = serde_json::json!({
            "gameLength": 1386200,
            "lastGameChunkId": 71,
            "lastKeyFrameId": 35,
            "statsJson": stats.to_string(),
        });
        let bytes = outer.to_string();
        let m = Metadata::parse(bytes.as_bytes()).unwrap();
        assert_eq!(m.game_length_ms, 1_386_200);
        assert_eq!(m.last_game_chunk_id, 71);
        assert_eq!(m.last_key_frame_id, 35);
        assert_eq!(m.players.len(), 10);
        assert_eq!(m.players[0].skin, "Poppy");
        assert_eq!(m.players[0].team, Team::Blue);
        assert_eq!(m.players[0].role, Role::Top);
        assert!(m.players[0].win);
        assert_eq!(m.players[9].role, Role::Support);
        assert_eq!(m.winning_team, Team::Blue);
    }

    #[test]
    fn prefers_individual_position_when_present() {
        let stats = serde_json::json!([
            {"SKIN": "Jinx", "TEAM": "100", "WIN": "Win", "INDIVIDUAL_POSITION": "BOTTOM"},
        ]);
        let outer = serde_json::json!({
            "gameLength": 100,
            "lastGameChunkId": 0,
            "lastKeyFrameId": 0,
            "statsJson": stats.to_string(),
        });
        let m = Metadata::parse(outer.to_string().as_bytes()).unwrap();
        assert_eq!(m.players[0].role, Role::Adc);
    }

    #[test]
    fn losing_team_0_flips_winning_team() {
        let stats = serde_json::json!([
            {"SKIN": "Ahri", "TEAM": "100", "WIN": "Fail"},
        ]);
        let outer = serde_json::json!({
            "gameLength": 100,
            "lastGameChunkId": 0,
            "lastKeyFrameId": 0,
            "statsJson": stats.to_string(),
        });
        let m = Metadata::parse(outer.to_string().as_bytes()).unwrap();
        assert_eq!(m.winning_team, Team::Red);
    }

    #[test]
    fn missing_game_length_errors() {
        let outer = serde_json::json!({
            "lastGameChunkId": 0,
            "lastKeyFrameId": 0,
            "statsJson": "[]",
        });
        let err = Metadata::parse(outer.to_string().as_bytes()).unwrap_err();
        matches!(err, RoflError::MetadataMissingKey("gameLength"));
    }
}
