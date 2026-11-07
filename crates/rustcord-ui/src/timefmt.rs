use chrono::{DateTime, NaiveDate, TimeZone};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TimeFormat {
    pub clock_24h: bool,
    pub month_first: bool,
}

const TWELVE_HOUR_REGIONS: &[&str] = &["US", "CA", "AU", "NZ", "PH", "IN", "PK", "BD", "EG", "SA", "MY", "KR"];
const MONTH_FIRST_REGIONS: &[&str] = &["US", "PH", "FM", "MH", "PW"];

fn locale_region(locale: &str) -> Option<&str> {
    let tag = locale.split(['.', '@']).next()?;
    let region = tag.split(['_', '-']).nth(1)?;
    (region.len() == 2).then_some(region)
}

fn os_locale() -> Option<String> {
    ["LC_ALL", "LC_TIME", "LANG"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty())
}

impl TimeFormat {
    pub fn for_locale(locale: Option<&str>) -> Self {
        match locale.and_then(locale_region) {
            Some(region) => Self {
                clock_24h: !TWELVE_HOUR_REGIONS.contains(&region),
                month_first: MONTH_FIRST_REGIONS.contains(&region),
            },
            None => Self { clock_24h: false, month_first: true },
        }
    }

    pub fn resolve(clock_24h_setting: Option<bool>) -> Self {
        let os = Self::for_locale(os_locale().as_deref());
        Self { clock_24h: clock_24h_setting.unwrap_or(os.clock_24h), ..os }
    }

    pub fn time<Tz: TimeZone>(&self, dt: &DateTime<Tz>) -> String
    where
        Tz::Offset: std::fmt::Display,
    {
        if self.clock_24h { dt.format("%H:%M").to_string() } else { dt.format("%-I:%M %p").to_string() }
    }

    pub fn short_date<Tz: TimeZone>(&self, dt: &DateTime<Tz>) -> String
    where
        Tz::Offset: std::fmt::Display,
    {
        if self.month_first { dt.format("%-m/%-d/%y").to_string() } else { dt.format("%d/%m/%y").to_string() }
    }

    pub fn short_datetime<Tz: TimeZone>(&self, dt: &DateTime<Tz>) -> String
    where
        Tz::Offset: std::fmt::Display,
    {
        format!("{}, {}", self.short_date(dt), self.time(dt))
    }

    pub fn long_date(&self, date: NaiveDate) -> String {
        if self.month_first { date.format("%B %-d, %Y").to_string() } else { date.format("%-d %B %Y").to_string() }
    }

    pub fn full<Tz: TimeZone>(&self, dt: &DateTime<Tz>) -> String
    where
        Tz::Offset: std::fmt::Display,
    {
        let date = if self.month_first { dt.format("%A, %B %-d, %Y") } else { dt.format("%A, %-d %B %Y") };
        format!("{date} at {}", self.time(dt))
    }

    pub fn header<Tz: TimeZone>(&self, dt: &DateTime<Tz>, today: NaiveDate) -> String
    where
        Tz::Offset: std::fmt::Display,
    {
        let date = dt.date_naive();
        if date == today {
            format!("Today at {}", self.time(dt))
        } else if today.pred_opt() == Some(date) {
            format!("Yesterday at {}", self.time(dt))
        } else {
            self.short_datetime(dt)
        }
    }
}

pub fn local_today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn at(iso: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(iso).unwrap()
    }

    const US: TimeFormat = TimeFormat { clock_24h: false, month_first: true };

    #[test]
    fn today_yesterday_and_older_headers() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 6).unwrap();
        assert_eq!(US.header(&at("2026-11-06T16:29:00+00:00"), today), "Today at 4:29 PM");
        assert_eq!(US.header(&at("2026-11-05T16:29:00+00:00"), today), "Yesterday at 4:29 PM");
        assert_eq!(US.header(&at("2026-10-30T19:34:00+00:00"), today), "10/30/26, 7:34 PM");
    }

    #[test]
    fn a_late_message_rolls_from_today_to_yesterday_at_midnight() {
        let sent = at("2026-11-06T23:58:00+00:00");
        assert_eq!(US.header(&sent, NaiveDate::from_ymd_opt(2026, 11, 6).unwrap()), "Today at 11:58 PM");
        assert_eq!(US.header(&sent, NaiveDate::from_ymd_opt(2026, 11, 7).unwrap()), "Yesterday at 11:58 PM");
    }

    #[test]
    fn full_and_divider_formats() {
        assert_eq!(US.full(&at("2026-10-30T19:34:00+00:00")), "Friday, October 30, 2026 at 7:34 PM");
        assert_eq!(US.long_date(NaiveDate::from_ymd_opt(2026, 11, 6).unwrap()), "November 6, 2026");
    }

    #[test]
    fn twenty_four_hour_clock() {
        let fmt = TimeFormat { clock_24h: true, ..US };
        assert_eq!(fmt.time(&at("2026-11-06T16:29:00+00:00")), "16:29");
    }

    #[test]
    fn locale_detection() {
        assert_eq!(TimeFormat::for_locale(Some("en_US.UTF-8")), TimeFormat { clock_24h: false, month_first: true });
        assert_eq!(TimeFormat::for_locale(Some("it_IT.UTF-8")), TimeFormat { clock_24h: true, month_first: false });
        assert_eq!(TimeFormat::for_locale(Some("en_GB")), TimeFormat { clock_24h: true, month_first: false });
        assert_eq!(TimeFormat::for_locale(Some("C")), TimeFormat { clock_24h: false, month_first: true });
    }
}
