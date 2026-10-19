use std::fmt;

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use thiserror::Error;

/// Upper bound for the forward search; a valid expression always fires within this horizon.
const SEARCH_HORIZON_DAYS: i64 = 366 * 5;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScheduleError {
    #[error(
        "cron expression must have 5 fields (minute hour day-of-month month day-of-week), got {0}"
    )]
    FieldCount(usize),
    #[error("invalid cron field `{field}`: {message}")]
    InvalidField { field: String, message: String },
    #[error("unknown timezone `{0}`; use an IANA name such as `Asia/Tokyo` or `UTC`")]
    UnknownTimezone(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CronField {
    allowed: Vec<bool>,
    restricted: bool,
}

impl CronField {
    fn contains(&self, value: u32) -> bool {
        self.allowed.get(value as usize).copied().unwrap_or(false)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ObjectiveSchedule {
    expression: String,
    timezone: Tz,
    minutes: CronField,
    hours: CronField,
    days_of_month: CronField,
    months: CronField,
    days_of_week: CronField,
}

impl fmt::Debug for ObjectiveSchedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ObjectiveSchedule({} @ {})",
            self.expression, self.timezone
        )
    }
}

impl ObjectiveSchedule {
    pub fn parse(expression: &str, timezone: &str) -> Result<Self, ScheduleError> {
        let timezone: Tz = timezone
            .trim()
            .parse()
            .map_err(|_| ScheduleError::UnknownTimezone(timezone.to_string()))?;
        let normalized = expand_alias(expression.trim());
        let fields: Vec<&str> = normalized.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(ScheduleError::FieldCount(fields.len()));
        }

        let mut days_of_week = parse_field(fields[4], 0, 7, WEEKDAY_NAMES)?;
        if days_of_week.contains(7) {
            days_of_week.allowed[0] = true;
        }

        Ok(Self {
            expression: expression.trim().to_string(),
            timezone,
            minutes: parse_field(fields[0], 0, 59, &[])?,
            hours: parse_field(fields[1], 0, 23, &[])?,
            days_of_month: parse_field(fields[2], 1, 31, &[])?,
            months: parse_field(fields[3], 1, 12, MONTH_NAMES)?,
            days_of_week,
        })
    }

    /// The first fire time strictly after `after`.
    #[must_use]
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let local_after = after.with_timezone(&self.timezone);
        let start_date = local_after.date_naive();
        for day_offset in 0..SEARCH_HORIZON_DAYS {
            let date = start_date + Duration::days(day_offset);
            if !self.day_matches(date) {
                continue;
            }
            for hour in 0..24 {
                if !self.hours.contains(hour) {
                    continue;
                }
                for minute in 0..60 {
                    if !self.minutes.contains(minute) {
                        continue;
                    }
                    let Some(naive) = date.and_hms_opt(hour, minute, 0) else {
                        continue;
                    };
                    let candidate = match self.timezone.from_local_datetime(&naive) {
                        LocalResult::Single(value) | LocalResult::Ambiguous(value, _) => value,
                        LocalResult::None => continue,
                    };
                    let candidate = candidate.with_timezone(&Utc);
                    if candidate > after {
                        return Some(candidate);
                    }
                }
            }
        }
        None
    }

    /// The most recent fire time at or before `now`, searching no earlier than `not_before`.
    /// Used to coalesce missed runs into a single catch-up cycle after downtime.
    #[must_use]
    pub fn latest_at_or_before(
        &self,
        now: DateTime<Utc>,
        not_before: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        let mut cursor = not_before - Duration::seconds(1);
        let mut latest = None;
        while let Some(next) = self.next_after(cursor) {
            if next > now {
                break;
            }
            latest = Some(next);
            cursor = next;
        }
        latest
    }

    fn day_matches(&self, date: NaiveDate) -> bool {
        if !self.months.contains(date.month()) {
            return false;
        }
        let dom = self.days_of_month.contains(date.day());
        let dow = self
            .days_of_week
            .contains(date.weekday().num_days_from_sunday());
        match (self.days_of_month.restricted, self.days_of_week.restricted) {
            (true, true) => dom || dow,
            (true, false) => dom,
            (false, true) => dow,
            (false, false) => true,
        }
    }
}

const WEEKDAY_NAMES: &[(&str, u32)] = &[
    ("sun", 0),
    ("mon", 1),
    ("tue", 2),
    ("wed", 3),
    ("thu", 4),
    ("fri", 5),
    ("sat", 6),
];

const MONTH_NAMES: &[(&str, u32)] = &[
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("may", 5),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
];

fn expand_alias(expression: &str) -> String {
    match expression.to_ascii_lowercase().as_str() {
        "@hourly" => "0 * * * *".to_string(),
        "@daily" | "@midnight" => "0 0 * * *".to_string(),
        "@weekly" => "0 0 * * 0".to_string(),
        "@monthly" => "0 0 1 * *".to_string(),
        _ => expression.to_string(),
    }
}

fn parse_field(
    raw: &str,
    min: u32,
    max: u32,
    names: &[(&str, u32)],
) -> Result<CronField, ScheduleError> {
    let invalid = |message: String| ScheduleError::InvalidField {
        field: raw.to_string(),
        message,
    };
    let mut allowed = vec![false; max as usize + 1];
    let restricted = raw != "*" && raw != "?";

    for part in raw.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step
                    .parse()
                    .map_err(|_| invalid(format!("step `{step}` is not a number")))?;
                if step == 0 {
                    return Err(invalid("step must be positive".to_string()));
                }
                (range, step)
            }
            None => (part, 1),
        };
        let (start, end) = if range == "*" || range == "?" {
            (min, max)
        } else if let Some((start, end)) = range.split_once('-') {
            (
                parse_value(start, names).ok_or_else(|| invalid(format!("bad value `{start}`")))?,
                parse_value(end, names).ok_or_else(|| invalid(format!("bad value `{end}`")))?,
            )
        } else {
            let start =
                parse_value(range, names).ok_or_else(|| invalid(format!("bad value `{range}`")))?;
            (start, if part.contains('/') { max } else { start })
        };
        if start < min || end > max || start > end {
            return Err(invalid(format!("range must be within {min}-{max}")));
        }
        let mut value = start;
        while value <= end {
            allowed[value as usize] = true;
            value += step;
        }
    }

    Ok(CronField {
        allowed,
        restricted,
    })
}

fn parse_value(raw: &str, names: &[(&str, u32)]) -> Option<u32> {
    raw.parse().ok().or_else(|| {
        let lower = raw.to_ascii_lowercase();
        names
            .iter()
            .find(|(name, _)| *name == lower)
            .map(|(_, value)| *value)
    })
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::{ObjectiveSchedule, ScheduleError};

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("parse time")
            .with_timezone(&Utc)
    }

    #[test]
    fn next_fire_every_six_hours_in_tokyo() {
        let schedule = ObjectiveSchedule::parse("0 */6 * * *", "Asia/Tokyo").expect("parse");

        // 2026-10-15T01:30Z is 10:30 JST → next is 12:00 JST = 03:00Z
        assert_eq!(
            schedule.next_after(at("2026-10-15T01:30:00Z")),
            Some(at("2026-10-15T03:00:00Z"))
        );
        // exactly on a fire time → strictly after
        assert_eq!(
            schedule.next_after(at("2026-10-15T03:00:00Z")),
            Some(at("2026-10-15T09:00:00Z"))
        );
    }

    #[test]
    fn supports_lists_ranges_names_and_aliases() {
        let weekdays = ObjectiveSchedule::parse("30 9 * * mon-fri", "UTC").expect("parse");
        // 2026-10-17 is a Saturday
        assert_eq!(
            weekdays.next_after(at("2026-10-16T10:00:00Z")),
            Some(at("2026-10-19T09:30:00Z"))
        );

        let daily = ObjectiveSchedule::parse("@daily", "UTC").expect("parse");
        assert_eq!(
            daily.next_after(at("2026-10-15T10:00:00Z")),
            Some(at("2026-10-16T00:00:00Z"))
        );

        let list = ObjectiveSchedule::parse("0,15 1 * * 7", "UTC").expect("parse");
        // 2026-10-18 is a Sunday; 7 means Sunday too
        assert_eq!(
            list.next_after(at("2026-10-18T01:00:00Z")),
            Some(at("2026-10-18T01:15:00Z"))
        );
    }

    #[test]
    fn dom_and_dow_use_or_semantics_when_both_restricted() {
        let schedule = ObjectiveSchedule::parse("0 0 1 * mon", "UTC").expect("parse");

        // From Thu 2026-10-15: next Monday 2026-10-19 comes before Nov 1
        assert_eq!(
            schedule.next_after(at("2026-10-15T00:00:00Z")),
            Some(at("2026-10-19T00:00:00Z"))
        );
    }

    #[test]
    fn latest_at_or_before_coalesces_missed_runs() {
        let schedule = ObjectiveSchedule::parse("0 * * * *", "UTC").expect("parse");

        assert_eq!(
            schedule.latest_at_or_before(at("2026-10-15T05:30:00Z"), at("2026-10-15T01:00:00Z")),
            Some(at("2026-10-15T05:00:00Z"))
        );
        assert_eq!(
            schedule.latest_at_or_before(at("2026-10-15T00:59:00Z"), at("2026-10-15T00:30:00Z")),
            None
        );
    }

    #[test]
    fn rejects_invalid_expressions() {
        assert_eq!(
            ObjectiveSchedule::parse("* * *", "UTC"),
            Err(ScheduleError::FieldCount(3))
        );
        assert!(matches!(
            ObjectiveSchedule::parse("61 * * * *", "UTC"),
            Err(ScheduleError::InvalidField { .. })
        ));
        assert!(matches!(
            ObjectiveSchedule::parse("*/0 * * * *", "UTC"),
            Err(ScheduleError::InvalidField { .. })
        ));
        assert_eq!(
            ObjectiveSchedule::parse("* * * * *", "Mars/Olympus"),
            Err(ScheduleError::UnknownTimezone("Mars/Olympus".to_string()))
        );
    }
}
