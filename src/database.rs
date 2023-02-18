use std::path::Path;

use chrono::{DateTime, Duration, Local};
use rusqlite::{Connection, OptionalExtension, Result};

pub trait TimeProvider {
    fn now(&self) -> DateTime<chrono::Utc>;
}

impl TimeProvider for chrono::Utc {
    fn now(&self) -> DateTime<chrono::Utc> {
        chrono::Utc::now()
    }
}
pub struct Database<'a, TP: TimeProvider> {
    conn: Connection,
    time_provider: &'a TP,
}

impl<'a, TP: TimeProvider> Database<'a, TP> {
    pub fn open<P: AsRef<Path>>(path: P, time_provider: &'a TP) -> Result<Self> {
        let conn = Connection::open(path)?;
        let s = Database {
            conn,
            time_provider,
        };
        s.conn.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_FKEY,
            true,
        )?;

        s.create_default_entries()?;
        s.add_work_end_at_shutdown()?;
        s.fix_missing_expected()?;

        Ok(s)
    }

    pub fn get_kv(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM key_value WHERE key=?;", [key], |row| {
                row.get(0)
            })
            .optional()
    }

    fn create_default_entries(&self) -> Result<()> {
        self.conn.execute("CREATE TABLE IF NOT EXISTS work_items (id INTEGER PRIMARY KEY ASC, name TEXT NOT NULL UNIQUE, description TEXT, visible BOOLEAN NOT NULL);", ())?;
        self.conn.execute("CREATE TABLE IF NOT EXISTS work_times (start TEXT NOT NULL UNIQUE, work_item INTEGER, FOREIGN KEY (work_item) REFERENCES work_items (id));", ())?;
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS key_value (key TEXT PRIMARY KEY, value TEXT);",
            (),
        )?;
        self.conn.execute("CREATE TABLE IF NOT EXISTS expected_time (date STRING PRIMARY KEY ASC, seconds INTEGER);", ())?;
        self.conn.execute("INSERT OR IGNORE INTO work_items(name, description, visible) VALUES ('Standup',NULL,1);", ())?;
        self.conn.execute(
            "INSERT OR IGNORE INTO key_value(key, value) VALUES ('default_time', 7*60*60);",
            (),
        )?;
        Ok(())
    }
    fn add_work_end_at_shutdown(&self) -> Result<()> {
        // Check if time of last shutdown was yesterday or earlier. Then add shutdown time as end of workday if no end was inserted before
        let last_shutdown: Option<String> = self.conn.query_row("SELECT value FROM key_value WHERE key='shutdown' AND date(?,'localtime')>date(value,'localtime');",(self.time_provider.now(),),|row| row.get(0),).optional().unwrap();
        if let Some(shutdown_time) = last_shutdown {
            let last_work:Option<u64>=self.conn.query_row("SELECT work_item FROM work_times WHERE date(start,'localtime')=date(?,'localtime') ORDER BY start DESC LIMIT 1", [&shutdown_time], |row| row.get(0)).optional().unwrap().flatten();
            if last_work.is_some() {
                self.conn.execute(
                    "INSERT INTO work_times (start,work_item) VALUES (?,NULL)",
                    [&shutdown_time],
                )?;
            }
        }
        Ok(())
    }
    fn fix_missing_expected(&self) -> Result<()> {
        let default_time = self
            .get_kv("default_time")?
            .and_then(|x| x.parse().ok())
            .unwrap_or(666);
        let mut stmt = self.conn.prepare(
            "SELECT date(start,'localtime') from work_times GROUP BY date(start,'localtime');",
        )?;
        let res = stmt.query_map((), |row| row.get::<_, chrono::NaiveDate>(0))?;
        for date in res.flatten() {
            self.conn.execute(
                "INSERT OR IGNORE INTO expected_time(date, seconds) VALUES (?, ?);",
                (&date, &default_time),
            )?;
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO expected_time(date, seconds) VALUES (date(?,'localtime'), ?);",
            (self.time_provider.now(), &default_time),
        )?;
        Ok(())
    }
    pub fn add_work_item(&self, name: &str) -> Result<usize> {
        self.conn.execute(
            "INSERT OR IGNORE INTO work_items(name, description, visible) VALUES (?,NULL,1);",
            [name],
        )
    }
    pub fn shutdown(&self) -> Result<()> {
        self.conn.execute("INSERT INTO key_value(key, value) VALUES ('shutdown', ?) ON CONFLICT DO UPDATE SET value=excluded.value;", (self.time_provider.now(),))?;
        Ok(())
    }
    pub fn get_available_work(&self) -> Result<Vec<(String, u64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name,id FROM work_items WHERE visible=1")?;
        let res = stmt.query_map((), |row| Ok((row.get(0)?, row.get(1)?)))?;
        res.collect()
    }
    pub fn get_current_work(&self) -> Result<Option<u64>> {
        self.conn.query_row("SELECT work_item FROM work_times WHERE date(start,'localtime')=date(?,'localtime') ORDER BY start DESC LIMIT 1", (self.time_provider.now(),), |row| row.get(0)).optional().map(|x| x.flatten())
    }
    pub fn set_current_work(&self, work_item: Option<u64>) -> Result<()> {
        self.conn.execute("INSERT INTO work_times (start,work_item) VALUES (?,?) ON CONFLICT DO UPDATE SET work_item=excluded.work_item;", (self.time_provider.now(),work_item))?;
        Ok(())
    }
    pub fn get_work_today(&self) -> Result<Vec<(Option<u64>, DateTime<Local>)>> {
        let mut stmt=self.conn.prepare("SELECT work_item,start FROM work_times WHERE date(start,'localtime')=date(?,'localtime') ORDER BY start ASC;")?;
        let res = stmt.query_map((self.time_provider.now(),), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        res.collect()
    }
    pub fn get_time_diff(&self) -> Result<Duration> {
        let mut total_time = self
            .get_kv("account_start")?
            .and_then(|x| x.parse().ok())
            .map(Duration::seconds)
            .unwrap_or_else(Duration::zero);
        let expected = self.conn.query_row(
            "SELECT coalesce(sum(seconds), 0) FROM expected_time WHERE \"date\"<date(?,'localtime');",
            (self.time_provider.now(),),
            |row| row.get(0),
        )?;
        let expected = Duration::seconds(expected);
        let mut stmt=self.conn.prepare("SELECT work_item,start FROM work_times WHERE date(start,'localtime')<date(?,'localtime') ORDER BY start ASC;")?;
        let res = stmt.query_map((self.time_provider.now(),), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        let mut current_time = None;
        for r in res {
            let (item, start) = r?;
            let item: Option<u64> = item;
            let start: chrono::DateTime<Local> = start;
            if let Some(ctime) = current_time {
                total_time = total_time + (start - ctime);
            }
            if item.is_some() {
                current_time = Some(start);
            }
        }
        Ok(total_time - expected)
    }
    pub fn get_expected_today(&self) -> Result<Duration> {
        self.conn
            .query_row(
                "SELECT seconds FROM expected_time WHERE date(\"date\")=date(?,'localtime')",
                (self.time_provider.now(),),
                |row| row.get(0),
            )
            .map(Duration::seconds)
    }
}

impl<TP: TimeProvider> Drop for Database<'_, TP> {
    fn drop(&mut self) {
        self.shutdown().ok();
    }
}

#[cfg(test)]
mod tests {
    use super::{Database, TimeProvider};
    use chrono::{Duration, Local, TimeZone};
    use std::collections::HashSet;

    struct MockTime {
        time: std::cell::RefCell<chrono::DateTime<chrono::Utc>>,
    }

    impl MockTime {
        fn new() -> Self {
            let time = chrono::Utc.with_ymd_and_hms(1990, 1, 1, 9, 0, 0).unwrap();
            MockTime {
                time: std::cell::RefCell::new(time),
            }
        }
        fn advance(&self, hours: i64) {
            self.time
                .replace_with(|t| *t + chrono::Duration::hours(hours));
        }
    }

    impl TimeProvider for MockTime {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            self.time.borrow().clone()
        }
    }

    #[test]
    fn time_mock() {
        let t = MockTime::new();
        let t1 = t.now();
        t.advance(1);
        let t2 = t.now();
        assert_eq!(t1 + Duration::hours(1), t2);
    }

    #[test]
    fn add_get_work_item() {
        let db = Database::open(":memory:", &chrono::Utc).unwrap();
        let work: HashSet<_> = db.get_available_work().unwrap().into_iter().collect();
        db.add_work_item("testwork").unwrap();
        let work2: HashSet<_> = db.get_available_work().unwrap().into_iter().collect();
        assert_eq!(work.len() + 1, work2.len(), "Wrong number of items added");
        assert!(work2.is_superset(&work), "Items missing after add");
        assert!(
            work2.iter().find(|x| x.0 == "testwork").is_some(),
            "Added item not found"
        );
    }

    #[test]
    fn get_set_current_work() {
        let t = MockTime::new();
        let db = Database::open(":memory:", &t).unwrap();
        assert_eq!(db.get_current_work(), Ok(None));
        let work_item = db.get_available_work().unwrap().first().unwrap().1;
        db.set_current_work(Some(work_item)).unwrap();
        t.advance(1);
        assert_eq!(db.get_current_work().unwrap(), Some(work_item));
    }

    #[test]
    fn time_diff() {
        let t = MockTime::new();
        let db = Database::open(":memory:", &t).unwrap();
        t.advance(-24);
        let work_item = db.get_available_work().unwrap().first().unwrap().1;
        db.set_current_work(Some(work_item)).unwrap();
        t.advance(5);
        db.set_current_work(None).unwrap();
        t.advance(19);
        db.fix_missing_expected().unwrap();
        let diff = db.get_time_diff().unwrap();
        let expected = db.get_expected_today().unwrap();
        assert_eq!(diff, Duration::hours(5) - expected);
        db.conn
            .execute(
                "INSERT INTO key_value(key, value) VALUES ('account_start', 3*60*60);",
                (),
            )
            .unwrap();
        let diff = db.get_time_diff().unwrap();
        assert_eq!(diff, Duration::hours(5 + 3) - expected);
    }

    #[test]
    fn shutdown() {
        let t = MockTime::new();
        let db = Database::open(":memory:", &t).unwrap();
        let work_item = Some(db.get_available_work().unwrap().first().unwrap().1);
        db.set_current_work(work_item).unwrap();
        let start_time = t.now().with_timezone(&Local);
        t.advance(1);
        let end_time = t.now().with_timezone(&Local);
        db.shutdown().unwrap();
        db.add_work_end_at_shutdown().unwrap(); // Same day. Should do nothing
        let today = db.get_work_today().unwrap();
        assert_eq!(today, vec![(work_item, start_time)]);
        t.advance(24);
        db.add_work_end_at_shutdown().unwrap(); // Next day. Should add None
        t.advance(-24);
        let today = db.get_work_today().unwrap();
        assert_eq!(today, vec![(work_item, start_time), (None, end_time)]);
        t.advance(3);
        db.shutdown().unwrap();
        t.advance(24);
        db.add_work_end_at_shutdown().unwrap(); // Already None at day end. Should do nothing
        t.advance(-24);
        let today = db.get_work_today().unwrap();
        assert_eq!(today, vec![(work_item, start_time), (None, end_time)]);
    }
}
