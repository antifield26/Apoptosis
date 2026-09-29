//! Server bans: `banned-players.json` + `banned-ips.json` (P19-02).
//!
//! Vanilla's shape, beside `ops.json`: player entries carry
//! `{uuid, name, created, source, expires, reason}`, ip entries carry
//! `{ip, created, source, expires, reason}`. `expires` is `"forever"` or a
//! Vanilla-format date (`"2026-12-31 23:59:59 +0000"`); anything else is
//! refused rather than read as forever (forever is authority, never a
//! fallback). Policy mirrors [`crate::ops`] and [`crate::whitelist`]:
//!
//! - **Missing files are not an error.** Either file absent means that half
//!   bans nobody.
//! - **A malformed file is an error naming the file**, and the caller
//!   decides (`crate::lifecycle` logs and boots banning nobody).
//! - **The files stay Vanilla-pure**: written with exactly Vanilla's keys
//!   and date shape, so a Vanilla server boots on them and keeps every
//!   entry.
//! - **Expiry is clock-injected, never read from the wall here.** Every
//!   query takes `now: SystemTime`, so tests pin expiry with fixed times
//!   and production passes `SystemTime::now()`. An expired entry stays in
//!   the file but bans nobody; only a pardon removes rows.
//! - **No operator exemption.** Unlike the whitelist, a ban applies to
//!   operators too — that is Vanilla's behaviour, and exempting ops would
//!   make `/ban` unable to stop an abusive operator.

use mc_core::error::{ServerError, ServerResult};
use mc_data::json::{Limits, read_json};
use serde_json::Value;
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `banned-players.json` beside `ops.json`.
pub const BANNED_PLAYERS_FILE_NAME: &str = "banned-players.json";
/// `banned-ips.json` beside `ops.json`.
pub const BANNED_IPS_FILE_NAME: &str = "banned-ips.json";

/// Size and depth limits for either file (a handful of lines each).
pub const LIMITS: Limits = Limits {
    max_bytes: 1024 * 1024,
    max_depth: 16,
};

/// Vanilla's `expires` for a ban that never lapses.
pub const EXPIRES_FOREVER: &str = "forever";

/// Default reason when `/ban` names none (Vanilla's own default).
pub const DEFAULT_BAN_REASON: &str = "Banned by an operator.";

/// One banned profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerBan {
    /// Profile uuid, the key matching uses.
    pub uuid: String,
    /// Name as recorded. For messages; **not** used for matching.
    pub name: String,
    /// When the ban was placed.
    pub created: SystemTime,
    /// Who placed it (operator name).
    pub source: String,
    /// When it lapses, or `None` for forever.
    pub expires: Option<SystemTime>,
    /// Why, as shown on the ban screen.
    pub reason: String,
}

/// One banned address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpBan {
    /// The banned address.
    pub ip: IpAddr,
    /// When the ban was placed.
    pub created: SystemTime,
    /// Who placed it (operator name).
    pub source: String,
    /// When it lapses, or `None` for forever.
    pub expires: Option<SystemTime>,
    /// Why, as shown on the ban screen.
    pub reason: String,
}

/// Both ban lists, loaded once at boot like operators.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BanList {
    players: BTreeMap<String, PlayerBan>,
    ips: BTreeMap<IpAddr, IpBan>,
}

impl BanList {
    /// Empty lists: nobody is banned.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            players: BTreeMap::new(),
            ips: BTreeMap::new(),
        }
    }

    /// Load both files from a directory.
    ///
    /// A missing file yields that half empty, which is an ordinary server.
    /// A **malformed** file is an error naming it, returned rather than a
    /// partial list.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] when a present file cannot be
    /// understood; [`ServerError::Io`] when one exists and cannot be read.
    pub fn load(directory: &Path) -> ServerResult<Self> {
        let players = {
            let path = directory.join(BANNED_PLAYERS_FILE_NAME);
            if !path.exists() {
                BTreeMap::new()
            } else {
                let value = read_json(&path, LIMITS).map_err(ServerError::from)?;
                Self::players_from_value(&value, &path)?
            }
        };
        let ips = {
            let path = directory.join(BANNED_IPS_FILE_NAME);
            if !path.exists() {
                BTreeMap::new()
            } else {
                let value = read_json(&path, LIMITS).map_err(ServerError::from)?;
                Self::ips_from_value(&value, &path)?
            }
        };
        Ok(Self { players, ips })
    }

    /// Build the player half from an already-parsed JSON value.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] naming the file and the problem.
    pub fn players_from_value(
        value: &Value,
        path: &Path,
    ) -> ServerResult<BTreeMap<String, PlayerBan>> {
        let Some(entries) = value.as_array() else {
            return Err(ServerError::CorruptData(format!(
                "{}: a player ban list must be a JSON array of entries, found {}",
                path.display(),
                json_type_name(value)
            )));
        };
        let mut players = BTreeMap::new();
        for (index, entry) in entries.iter().enumerate() {
            let ban = player_entry(entry, path, index)?;
            if players.contains_key(&ban.uuid) {
                return Err(ServerError::CorruptData(format!(
                    "{}: entry {index} repeats uuid {}",
                    path.display(),
                    ban.uuid
                )));
            }
            players.insert(ban.uuid.clone(), ban);
        }
        Ok(players)
    }

    /// Build the ip half from an already-parsed JSON value.
    ///
    /// # Errors
    ///
    /// [`ServerError::CorruptData`] naming the file and the problem.
    pub fn ips_from_value(value: &Value, path: &Path) -> ServerResult<BTreeMap<IpAddr, IpBan>> {
        let Some(entries) = value.as_array() else {
            return Err(ServerError::CorruptData(format!(
                "{}: an ip ban list must be a JSON array of entries, found {}",
                path.display(),
                json_type_name(value)
            )));
        };
        let mut ips = BTreeMap::new();
        for (index, entry) in entries.iter().enumerate() {
            let ban = ip_entry(entry, path, index)?;
            if ips.contains_key(&ban.ip) {
                return Err(ServerError::CorruptData(format!(
                    "{}: entry {index} repeats ip {}",
                    path.display(),
                    ban.ip
                )));
            }
            ips.insert(ban.ip, ban);
        }
        Ok(ips)
    }

    /// Write both lists back in `directory`, Vanilla's shape.
    ///
    /// Player entries ascending by uuid, ip entries ascending by address,
    /// pretty-printed with exactly Vanilla's keys. A missing directory is
    /// created; anything else that goes wrong is an error the caller
    /// reports rather than a ban the caller pretends happened.
    ///
    /// # Errors
    ///
    /// [`ServerError::Operational`] when the directory cannot be created or
    /// a file cannot be written or serialised.
    pub fn save(&self, directory: &Path) -> ServerResult<()> {
        let players: Vec<serde_json::Value> = self
            .players
            .values()
            .map(|ban| {
                serde_json::json!({
                    "uuid": ban.uuid,
                    "name": ban.name,
                    "created": format_ban_time(ban.created),
                    "source": ban.source,
                    "expires": ban.expires.map_or_else(
                        || EXPIRES_FOREVER.to_owned(),
                        format_ban_time
                    ),
                    "reason": ban.reason,
                })
            })
            .collect();
        let ips: Vec<serde_json::Value> = self
            .ips
            .values()
            .map(|ban| {
                serde_json::json!({
                    "ip": ban.ip.to_string(),
                    "created": format_ban_time(ban.created),
                    "source": ban.source,
                    "expires": ban.expires.map_or_else(
                        || EXPIRES_FOREVER.to_owned(),
                        format_ban_time
                    ),
                    "reason": ban.reason,
                })
            })
            .collect();
        std::fs::create_dir_all(directory).map_err(|error| {
            ServerError::Operational(format!(
                "{}: cannot create directory: {error}",
                directory.display()
            ))
        })?;
        for (file, entries) in [
            (BANNED_PLAYERS_FILE_NAME, players),
            (BANNED_IPS_FILE_NAME, ips),
        ] {
            let text = serde_json::to_string_pretty(&entries).map_err(|error| {
                ServerError::Operational(format!("{file}: cannot serialise: {error}"))
            })?;
            std::fs::write(directory.join(file), text).map_err(|error| {
                ServerError::Operational(format!(
                    "{}: cannot write {file}: {error}",
                    directory.display()
                ))
            })?;
        }
        Ok(())
    }

    /// Ban `uuid` (insert or replace).
    ///
    /// Re-banning replaces the row (new source/reason/expiry): the file can
    /// never hold two rows for one profile.
    pub fn ban_player(&mut self, ban: PlayerBan) {
        let uuid = normalise_uuid(&ban.uuid);
        let mut ban = ban;
        ban.uuid = uuid.clone();
        self.players.insert(uuid, ban);
    }

    /// Pardon `uuid`. Returns whether a row was removed.
    pub fn unban_player(&mut self, uuid: &str) -> bool {
        self.players.remove(&normalise_uuid(uuid)).is_some()
    }

    /// Whether `uuid` is banned at `now`.
    ///
    /// An expired row bans nobody (it stays in the file until pardoned).
    /// Every accessor normalises the key, so case cannot hide a ban.
    #[must_use]
    pub fn is_player_banned_at(&self, uuid: &str, now: SystemTime) -> bool {
        self.players
            .get(&normalise_uuid(uuid))
            .is_some_and(|ban| ban.expires.is_none_or(|expires| now < expires))
    }

    /// The player row for a uuid, if any (regardless of expiry).
    #[must_use]
    pub fn player_ban(&self, uuid: &str) -> Option<&PlayerBan> {
        self.players.get(&normalise_uuid(uuid))
    }

    /// The ip row for an address, if any (regardless of expiry).
    #[must_use]
    pub fn ip_ban(&self, ip: &IpAddr) -> Option<&IpBan> {
        self.ips.get(ip)
    }

    /// Ban `ip` (insert or replace).
    pub fn ban_ip(&mut self, ban: IpBan) {
        self.ips.insert(ban.ip, ban);
    }

    /// Pardon `ip`. Returns whether a row was removed.
    pub fn unban_ip(&mut self, ip: &IpAddr) -> bool {
        self.ips.remove(ip).is_some()
    }

    /// Whether `ip` is banned at `now`.
    #[must_use]
    pub fn is_ip_banned_at(&self, ip: &IpAddr, now: SystemTime) -> bool {
        self.ips
            .get(ip)
            .is_some_and(|ban| ban.expires.is_none_or(|expires| now < expires))
    }

    /// How many player rows are held (regardless of expiry).
    #[must_use]
    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    /// How many ip rows are held (regardless of expiry).
    #[must_use]
    pub fn ip_count(&self) -> usize {
        self.ips.len()
    }

    /// Every player row, ascending by uuid.
    pub fn player_bans(&self) -> impl Iterator<Item = &PlayerBan> {
        self.players.values()
    }

    /// Every ip row, ascending by address.
    pub fn ip_bans(&self) -> impl Iterator<Item = &IpBan> {
        self.ips.values()
    }
}

/// A uuid as this module keys on it: trimmed and lower-case.
#[must_use]
fn normalise_uuid(uuid: &str) -> String {
    uuid.trim().to_ascii_lowercase()
}

/// Parse one player-ban entry.
fn player_entry(entry: &Value, path: &Path, index: usize) -> ServerResult<PlayerBan> {
    let Some(object) = entry.as_object() else {
        return Err(ServerError::CorruptData(format!(
            "{}: entry {index} is {}, not an object",
            path.display(),
            json_type_name(entry)
        )));
    };
    let uuid = object
        .get("uuid")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            ServerError::CorruptData(format!(
                "{p}: entry {index} has no string `uuid`",
                p = path.display()
            ))
        })?;
    let uuid = normalise_uuid(uuid);
    if uuid.is_empty() {
        return Err(ServerError::CorruptData(format!(
            "{}: entry {index} has an empty `uuid`",
            path.display()
        )));
    }
    let name = object
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    let created = required_time(object, path, index, "created")?;
    let source = object
        .get("source")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    let expires = required_expires(object, path, index)?;
    let reason = object
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    Ok(PlayerBan {
        uuid,
        name,
        created,
        source,
        expires,
        reason,
    })
}

/// Parse one ip-ban entry.
fn ip_entry(entry: &Value, path: &Path, index: usize) -> ServerResult<IpBan> {
    let Some(object) = entry.as_object() else {
        return Err(ServerError::CorruptData(format!(
            "{}: entry {index} is {}, not an object",
            path.display(),
            json_type_name(entry)
        )));
    };
    let ip = object
        .get("ip")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            ServerError::CorruptData(format!(
                "{p}: entry {index} has no string `ip`",
                p = path.display()
            ))
        })?;
    let ip: IpAddr = ip.trim().parse().map_err(|_| {
        ServerError::CorruptData(format!(
            "{}: entry {index} has ip {ip:?}, which is not an address",
            path.display()
        ))
    })?;
    let created = required_time(object, path, index, "created")?;
    let source = object
        .get("source")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    let expires = required_expires(object, path, index)?;
    let reason = object
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    Ok(IpBan {
        ip,
        created,
        source,
        expires,
        reason,
    })
}

/// A required date field: present, a string, and shaped like Vanilla's.
fn required_time(
    object: &serde_json::Map<String, Value>,
    path: &Path,
    index: usize,
    field: &str,
) -> ServerResult<SystemTime> {
    let Some(text) = object.get(field).and_then(serde_json::Value::as_str) else {
        return Err(ServerError::CorruptData(format!(
            "{p}: entry {index} has no string `{field}`",
            p = path.display()
        )));
    };
    parse_ban_time(text).ok_or_else(|| {
        ServerError::CorruptData(format!(
            "{p}: entry {index} has `{field}` {text:?}, which is not a Vanilla date",
            p = path.display()
        ))
    })
}

/// A required `expires` field: `"forever"` or a Vanilla date.
///
/// Anything else is refused: reading an unknown shape as forever would
/// grant permanent authority to a typo, and reading it as expired would
/// silently unban.
fn required_expires(
    object: &serde_json::Map<String, Value>,
    path: &Path,
    index: usize,
) -> ServerResult<Option<SystemTime>> {
    let Some(text) = object.get("expires").and_then(serde_json::Value::as_str) else {
        return Err(ServerError::CorruptData(format!(
            "{p}: entry {index} has no string `expires`",
            p = path.display()
        )));
    };
    if text == EXPIRES_FOREVER {
        return Ok(None);
    }
    parse_ban_time(text)
        .map(Some)
        .ok_or_else(|| {
            ServerError::CorruptData(format!(
                "{p}: entry {index} has `expires` {text:?}, which is neither \"forever\" nor a Vanilla date",
                p = path.display()
            ))
        })
}

/// Parse Vanilla's ban date: `"2026-12-31 23:59:59 +0000"`.
///
/// Strict and total: out-of-range months, days (leap-aware), hours,
/// minutes, seconds and offsets are all `None`. No date library is
/// involved — the shape is fixed, and a hand-rolled total parser is
/// reviewable where a lenient library call would not be.
#[must_use]
pub fn parse_ban_time(text: &str) -> Option<SystemTime> {
    let text = text.trim();
    let (date, rest) = text.split_once(' ')?;
    let (time, offset) = rest.split_once(' ')?;
    let (year, month, day) = split3(date, '-')?;
    let (hour, minute, second) = split3(time, ':')?;
    if !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let dim = days_in_month(year, month);
    if day < 1 || day > dim {
        return None;
    }
    let offset_minutes = parse_offset(offset)?;
    let days = days_from_civil(year, month, day);
    let seconds =
        days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second)
            - offset_minutes * 60;
    UNIX_EPOCH.checked_add(Duration::from_secs(seconds.max(0) as u64))
}

/// Format a time as Vanilla's ban date, always `+0000`.
#[must_use]
pub fn format_ban_time(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day, _, _, _) = civil_from_days((seconds / 86_400) as i64);
    let rest = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} +0000",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// Split `"aXbXc"` into three `i32`s.
fn split3(text: &str, sep: char) -> Option<(i32, i32, i32)> {
    let mut parts = text.split(sep);
    let a = parts.next()?.parse().ok()?;
    let b = parts.next()?.parse().ok()?;
    let c = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((a, b, c))
}

/// Days in a month, leap-aware.
fn days_in_month(year: i32, month: i32) -> i32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        _ => 28,
    }
}

/// Gregorian leap years.
const fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Days from 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let y = i64::from(if month <= 2 { year - 1 } else { year });
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(if month <= 2 { month + 9 } else { month - 3 });
    let doy = (153 * m + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil date and clock for days since the epoch (inverse of above).
fn civil_from_days(days: i64) -> (i32, i32, i32, i32, i32, i32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        i32::try_from(if m <= 2 { y + 1 } else { y }).unwrap_or(0),
        m as i32,
        d as i32,
        0,
        0,
        0,
    )
}

/// Parse `+HHMM` / `-HHMM` into minutes east of UTC.
fn parse_offset(offset: &str) -> Option<i64> {
    let (sign, digits) = match offset.as_bytes().first()? {
        b'+' => (1i64, &offset[1..]),
        b'-' => (-1i64, &offset[1..]),
        _ => return None,
    };
    if digits.len() != 4 {
        return None;
    }
    let hours: i64 = digits[..2].parse().ok()?;
    let minutes: i64 = digits[2..].parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 60 + minutes))
}

/// The JSON type name for refusal messages.
fn json_type_name(value: &Value) -> &'static str {
    if value.is_null() {
        "null"
    } else if value.is_boolean() {
        "a boolean"
    } else if value.is_number() {
        "a number"
    } else if value.is_string() {
        "a string"
    } else if value.is_array() {
        "an array"
    } else {
        "an object"
    }
}

#[cfg(test)]
mod tests {
    use super::{BanList, EXPIRES_FOREVER, IpBan, PlayerBan, format_ban_time, parse_ban_time};
    use mc_test_support::fixtures::TempDir;
    use std::net::IpAddr;
    use std::path::Path;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn fixed() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    #[test]
    fn dates_round_trip_in_vanillas_shape() {
        // 2023-11-14 22:13:20 UTC is the fixed instant above.
        assert_eq!(format_ban_time(fixed()), "2023-11-14 22:13:20 +0000");
        assert_eq!(parse_ban_time("2023-11-14 22:13:20 +0000"), Some(fixed()));
        assert_eq!(
            parse_ban_time("2023-11-14 22:13:20 -0500"),
            Some(fixed() + Duration::from_secs(5 * 3600))
        );
        // Malformed shapes are all refused, never read as forever.
        for bad in [
            "forever ",
            "FOREVER",
            "2023-13-01 00:00:00 +0000",
            "2023-02-29 00:00:00 +0000",
            "2024-02-29 00:00:00 +0000",
            "2023-11-14 24:00:00 +0000",
            "2023-11-14 22:13:20",
            "2023-11-14T22:13:20+00:00",
            "yesterday",
            "",
        ] {
            if bad == "2024-02-29 00:00:00 +0000" {
                assert!(parse_ban_time(bad).is_some(), "2024 is a leap year: {bad}");
                continue;
            }
            assert!(parse_ban_time(bad).is_none(), "refused: {bad}");
        }
    }

    #[test]
    fn expiry_is_clock_injected() {
        let now = fixed();
        let mut list = BanList::new();
        list.ban_player(PlayerBan {
            uuid: "069a79f4-44e9-4726-a5be-f4a7b64ac909".to_owned(),
            name: "Temp".to_owned(),
            created: now,
            source: "Chief".to_owned(),
            expires: Some(now + Duration::from_secs(60)),
            reason: "test".to_owned(),
        });
        assert!(list.is_player_banned_at("069a79f4-44e9-4726-a5be-f4a7b64ac909", now));
        assert!(
            !list.is_player_banned_at(
                "069a79f4-44e9-4726-a5be-f4a7b64ac909",
                now + Duration::from_secs(61)
            ),
            "an expired row bans nobody"
        );
        // Forever never lapses.
        list.ban_player(PlayerBan {
            uuid: "11111111-1111-1111-1111-111111111111".to_owned(),
            name: "Perm".to_owned(),
            created: now,
            source: "Chief".to_owned(),
            expires: None,
            reason: "test".to_owned(),
        });
        assert!(list.is_player_banned_at(
            "11111111-1111-1111-1111-111111111111",
            now + Duration::from_secs(4_000_000_000)
        ));
    }

    #[test]
    fn ip_bans_match_by_address() {
        let now = fixed();
        let mut list = BanList::new();
        let ip: IpAddr = "203.0.113.7".parse().expect("test ip");
        list.ban_ip(IpBan {
            ip,
            created: now,
            source: "Chief".to_owned(),
            expires: None,
            reason: "test".to_owned(),
        });
        assert!(list.is_ip_banned_at(&ip, now));
        let other: IpAddr = "203.0.113.8".parse().expect("test ip");
        assert!(!list.is_ip_banned_at(&other, now));
    }

    #[test]
    fn files_round_trip_with_vanillas_keys() {
        let dir = TempDir::new("bans-round-trip");
        let now = fixed();
        let mut list = BanList::new();
        list.ban_player(PlayerBan {
            uuid: "069a79f4-44e9-4726-a5be-f4a7b64ac909".to_owned(),
            name: "Notch".to_owned(),
            created: now,
            source: "Chief".to_owned(),
            expires: None,
            reason: "griefing".to_owned(),
        });
        list.ban_ip(IpBan {
            ip: "203.0.113.7".parse().expect("test ip"),
            created: now,
            source: "Chief".to_owned(),
            expires: Some(now + Duration::from_secs(3600)),
            reason: "spam".to_owned(),
        });
        list.save(dir.path()).expect("saves");
        let back = BanList::load(dir.path()).expect("reloads");
        assert_eq!(back, list);
        // Exactly Vanilla's keys on each side.
        let players: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join(super::BANNED_PLAYERS_FILE_NAME))
                .expect("reads"),
        )
        .expect("json");
        let mut keys: Vec<&str> = players[0]
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["created", "expires", "name", "reason", "source", "uuid"]
        );
        let ips: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join(super::BANNED_IPS_FILE_NAME)).expect("reads"),
        )
        .expect("json");
        assert_eq!(
            ips[0].get("expires").and_then(serde_json::Value::as_str),
            Some("2023-11-14 23:13:20 +0000"),
            "a temporary ban writes a Vanilla date"
        );
        assert_eq!(
            ips[0].get("ip").and_then(serde_json::Value::as_str),
            Some("203.0.113.7")
        );
    }

    #[test]
    fn malformed_files_are_refused_by_name() {
        let dir = TempDir::new("bans-malformed");
        std::fs::write(
            dir.path().join(super::BANNED_PLAYERS_FILE_NAME),
            r#"[{"name": "NoUuid"}]"#,
        )
        .expect("fixture");
        assert!(
            BanList::load(dir.path()).is_err(),
            "a player row without uuid is refused"
        );
        std::fs::write(
            dir.path().join(super::BANNED_IPS_FILE_NAME),
            r#"[{"ip": "not-an-address"}]"#,
        )
        .expect("fixture");
        assert!(BanList::load(dir.path()).is_err());
        // A file that parses but is not an array is refused too.
        assert!(
            BanList::players_from_value(
                &serde_json::json!({"uuid": "x"}),
                Path::new("banned-players.json")
            )
            .is_err()
        );
        assert_eq!(EXPIRES_FOREVER, "forever");
    }
}
