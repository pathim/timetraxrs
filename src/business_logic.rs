use crate::database::{Database, TimeProvider};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate};
use std::{collections::HashMap, num::ParseIntError};

#[derive(Debug, PartialEq)]
pub enum Error {
    Inconsistent(NaiveDate),
    InvalidValue(ParseIntError),
    DbError(crate::database::Error),
}
impl From<crate::database::Error> for Error {
    fn from(value: crate::database::Error) -> Self {
        Self::DbError(value)
    }
}
impl From<ParseIntError> for Error {
    fn from(value: ParseIntError) -> Self {
        Self::InvalidValue(value)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inconsistent(d) => write!(f, "Inconsistent data on {}. No end of workday?", d),
            Self::InvalidValue(e) => e.fmt(f),
            Self::DbError(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}
struct DateRange {
    end: NaiveDate,
    current: NaiveDate,
}
impl DateRange {
    pub fn new(start: NaiveDate, end: NaiveDate) -> Self {
        Self {
            current: start,
            end,
        }
    }
}
impl Iterator for DateRange {
    type Item = NaiveDate;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current == self.end {
            return None;
        }
        let res = self.current;
        self.current += chrono::Duration::days(1);
        Some(res)
    }
}

pub fn get_default_time<T: TimeProvider>(
    db: &Database<T>,
    date: NaiveDate,
) -> Result<i64, rusqlite::Error> {
    match date.weekday() {
        chrono::Weekday::Sat | chrono::Weekday::Sun => {
            return Ok(0);
        }
        _ => {}
    }
    if holiday_de::GermanRegion::BadenWuerttemberg.is_holiday(date) {
        return Ok(0);
    }
    let default_time = db.get_kv::<i64>("default_time")?;
    Ok(default_time)
}

pub fn get_expected_work_or_insert_default<T: TimeProvider>(
    db: &Database<T>,
    date: NaiveDate,
) -> Result<Duration, Error> {
    Ok(if let Some(expected) = db.get_expected_work(date)? {
        expected
    } else {
        let time = get_default_time(db, date)?;
        db.set_expected_time(date, time)?;
        Duration::seconds(time)
    })
}

#[derive(Debug, PartialEq)]
pub struct WorkdayTime {
    pub work_done: Result<Duration, Error>,
    pub expected: Duration,
}

pub fn get_work_time_by_day<T: TimeProvider>(
    db: &Database<T>,
) -> Result<HashMap<NaiveDate, WorkdayTime>, Error> {
    let mut result = HashMap::new();
    if let Some(start_day) = db.get_start_day()? {
        let today = db.now().with_timezone(&Local).date_naive();
        for date in DateRange::new(start_day, today) {
            let work_done = db
                .get_work_on_date(&date)
                .map_err(Into::into)
                .and_then(|x| work_times_to_duration(&x));
            let expected = get_expected_work_or_insert_default(db, date)?;
            result.insert(
                date,
                WorkdayTime {
                    work_done,
                    expected,
                },
            );
        }
    }
    Ok(result)
}

fn work_times_to_duration(times: &[(Option<u64>, DateTime<Local>)]) -> Result<Duration, Error> {
    if let Some(last) = times.last() {
        if last.0.is_some() {
            // Last entry of the day should be NULL
            return Err(Error::Inconsistent(last.1.date_naive()));
        }
    } else {
        return Ok(Duration::zero());
    }
    let mut res = Duration::zero();
    for work in times.windows(2) {
        let start = &work[0];
        let end = &work[1];
        if start.0.is_some() {
            res = res + (end.1 - start.1);
        }
    }
    Ok(res)
}

pub fn time_diff<T: TimeProvider>(db: &Database<T>) -> Result<Duration, Error> {
    let times = get_work_time_by_day(db)?;
    let mut res = Duration::zero();
    for (_, workday_time) in times {
        let work = workday_time.work_done?;
        res = res + work - workday_time.expected;
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::{get_work_time_by_day, work_times_to_duration};
    use super::{Database, WorkdayTime};
    use crate::database::{tests::MockTime, TimeProvider};
    use chrono::{Duration, NaiveDate, TimeZone};
    #[test]
    fn test_work_times_to_duration() {
        assert_eq!(
            work_times_to_duration(&[]).unwrap(),
            chrono::Duration::zero(),
            "Empty work times"
        );
        assert_eq!(
            work_times_to_duration(&vec![(
                Some(1),
                chrono::Local.with_ymd_and_hms(2000, 1, 1, 9, 0, 0).unwrap()
            )]),
            Err(super::Error::Inconsistent(
                NaiveDate::from_ymd_opt(2000, 1, 1).unwrap()
            )),
            "Single value, no end time"
        );
        assert_eq!(
            work_times_to_duration(&vec![
                (
                    Some(1),
                    chrono::Local.with_ymd_and_hms(2000, 1, 1, 9, 0, 0).unwrap()
                ),
                (
                    Some(1),
                    chrono::Local
                        .with_ymd_and_hms(2000, 1, 1, 10, 0, 0)
                        .unwrap()
                ),
            ]),
            Err(super::Error::Inconsistent(
                NaiveDate::from_ymd_opt(2000, 1, 1).unwrap()
            )),
            "Two values, no end time"
        );
        assert_eq!(
            work_times_to_duration(&vec![
                (
                    Some(1),
                    chrono::Local.with_ymd_and_hms(2000, 1, 1, 9, 0, 0).unwrap()
                ),
                (
                    None,
                    chrono::Local
                        .with_ymd_and_hms(2000, 1, 1, 9, 30, 0)
                        .unwrap()
                ),
                (
                    Some(1),
                    chrono::Local
                        .with_ymd_and_hms(2000, 1, 1, 10, 0, 0)
                        .unwrap()
                ),
            ]),
            Err(super::Error::Inconsistent(
                NaiveDate::from_ymd_opt(2000, 1, 1).unwrap()
            )),
            "Two values, no end time, with break"
        );
        assert_eq!(
            work_times_to_duration(&vec![
                (
                    Some(1),
                    chrono::Local.with_ymd_and_hms(2000, 1, 1, 9, 0, 0).unwrap()
                ),
                (
                    None,
                    chrono::Local
                        .with_ymd_and_hms(2000, 1, 1, 10, 0, 0)
                        .unwrap()
                ),
            ])
            .unwrap(),
            chrono::Duration::hours(1),
            "Single value, with end time"
        );
        assert_eq!(
            work_times_to_duration(&vec![
                (
                    Some(1),
                    chrono::Local.with_ymd_and_hms(2000, 1, 1, 9, 0, 0).unwrap()
                ),
                (
                    Some(2),
                    chrono::Local
                        .with_ymd_and_hms(2000, 1, 1, 10, 0, 0)
                        .unwrap()
                ),
                (
                    None,
                    chrono::Local
                        .with_ymd_and_hms(2000, 1, 1, 11, 0, 0)
                        .unwrap()
                ),
            ])
            .unwrap(),
            chrono::Duration::hours(2),
            "Two values, with end time"
        );
        assert_eq!(
            work_times_to_duration(&vec![
                (
                    Some(1),
                    chrono::Local.with_ymd_and_hms(2000, 1, 1, 9, 0, 0).unwrap()
                ),
                (
                    None,
                    chrono::Local
                        .with_ymd_and_hms(2000, 1, 1, 10, 0, 0)
                        .unwrap()
                ),
                (
                    Some(2),
                    chrono::Local
                        .with_ymd_and_hms(2000, 1, 1, 10, 30, 0)
                        .unwrap()
                ),
                (
                    None,
                    chrono::Local
                        .with_ymd_and_hms(2000, 1, 1, 11, 30, 0)
                        .unwrap()
                ),
            ])
            .unwrap(),
            chrono::Duration::hours(2),
            "Two values, with end time, and break"
        );
    }
    #[test]
    fn test_get_work_time_by_day() {
        let t = MockTime::new();
        let db = Database::open(":memory:", &t).unwrap();
        db.add_work_item("test").unwrap();
        let start = t.now().date_naive();
        let work_item = db.get_available_work().unwrap().first().unwrap().1;
        db.set_current_work(Some(work_item)).unwrap();
        db.set_expected_time(t.now().date_naive(), 5 * 60 * 60)
            .unwrap();
        t.advance(1);
        db.set_current_work(None).unwrap();
        t.advance(23);
        db.set_expected_time(t.now().date_naive(), 6 * 60 * 60)
            .unwrap();
        db.set_current_work(Some(work_item)).unwrap();
        t.advance(2);
        db.set_current_work(None).unwrap();
        t.advance(22);
        db.set_expected_time(t.now().date_naive(), 7 * 60 * 60)
            .unwrap();
        db.set_current_work(Some(work_item)).unwrap();
        t.advance(23);
        db.set_expected_time(t.now().date_naive(), 8 * 60 * 60)
            .unwrap();
        db.set_current_work(Some(work_item)).unwrap();
        let day = Duration::days(1);
        let expected = std::collections::HashMap::from_iter([
            (
                start,
                WorkdayTime {
                    work_done: Ok(Duration::hours(1)),
                    expected: Duration::hours(5),
                },
            ),
            (
                start + day,
                WorkdayTime {
                    work_done: Ok(Duration::hours(2)),
                    expected: Duration::hours(6),
                },
            ),
            (
                start + day * 2,
                WorkdayTime {
                    work_done: Err(super::Error::Inconsistent(start + day * 2)),
                    expected: Duration::hours(7),
                },
            ),
        ]);
        let res = get_work_time_by_day(&db).unwrap();
        assert_eq!(res, expected);
    }
}
