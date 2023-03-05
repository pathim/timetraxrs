use std::path::Path;

use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
pub use rusqlite::Result;
use rusqlite::{Connection, OptionalExtension};
pub type Error = rusqlite::Error;

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

        Ok(s)
    }

    pub fn get_kv<T: rusqlite::types::FromSql>(&self, key: &str) -> Result<T> {
        self.conn
            .query_row("SELECT value FROM key_value WHERE key=?;", [key], |row| {
                row.get(0)
            })
    }

    fn create_default_entries(&self) -> Result<()> {
        self.conn.execute("CREATE TABLE IF NOT EXISTS work_items (id INTEGER PRIMARY KEY ASC, name TEXT NOT NULL UNIQUE, description TEXT, visible BOOLEAN NOT NULL);", ())?;
        self.conn.execute("CREATE TABLE IF NOT EXISTS work_times (start TEXT NOT NULL UNIQUE, work_item INTEGER, FOREIGN KEY (work_item) REFERENCES work_items (id));", ())?;
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS key_value (key TEXT PRIMARY KEY, value ANY);",
            (),
        )?;
        self.conn.execute("CREATE TABLE IF NOT EXISTS expected_time (date STRING PRIMARY KEY ASC, seconds INTEGER);", ())?;
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
    pub fn set_expected_time(&self, date: NaiveDate, time_s: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO expected_time(date, seconds) VALUES (?, ?) ON CONFLICT DO UPDATE SET seconds=excluded.seconds;",
            (&date, &time_s),
        )?;
        Ok(())
    }
    pub fn get_expected_work(&self, date: NaiveDate) -> Result<Option<Duration>> {
        Ok(self
            .conn
            .query_row(
                "SELECT seconds FROM expected_time WHERE date(\"date\")=date(?)",
                (date,),
                |row| row.get(0),
            )
            .optional()?
            .map(Duration::seconds))
    }

    pub fn now(&self) -> chrono::DateTime<Utc> {
        self.time_provider.now()
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
    pub fn get_start_day(&self) -> Result<Option<chrono::NaiveDate>> {
        self.conn
            .query_row(
                "SELECT date(start,'localtime') FROM work_times ORDER BY start ASC LIMIT 1",
                (),
                |row| row.get(0),
            )
            .optional()
    }
    pub fn set_current_work(&self, work_item: Option<u64>) -> Result<()> {
        self.conn.execute("INSERT INTO work_times (start,work_item) VALUES (?,?) ON CONFLICT DO UPDATE SET work_item=excluded.work_item;", (self.time_provider.now(),work_item))?;
        Ok(())
    }
    pub fn get_work_on_date(
        &self,
        date: &chrono::NaiveDate,
    ) -> Result<Vec<(Option<u64>, DateTime<Local>)>> {
        let mut stmt=self.conn.prepare("SELECT work_item,start FROM work_times WHERE date(start,'localtime')=date(?) ORDER BY start ASC;")?;
        let res = stmt.query_map((date,), |row| Ok((row.get(0)?, row.get(1)?)))?;
        res.collect()
    }
}

impl<TP: TimeProvider> Drop for Database<'_, TP> {
    fn drop(&mut self) {
        self.shutdown().ok();
    }
}

#[cfg(test)]
pub mod tests {
    use super::{Database, TimeProvider};
    use chrono::{Duration, Local, TimeZone};
    use std::collections::HashSet;

    pub struct MockTime {
        time: std::cell::RefCell<chrono::DateTime<chrono::Utc>>,
    }

    impl MockTime {
        pub fn new() -> Self {
            let time = chrono::Utc.with_ymd_and_hms(1990, 1, 1, 9, 0, 0).unwrap();
            MockTime {
                time: std::cell::RefCell::new(time),
            }
        }
        pub fn advance(&self, hours: i64) {
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
        db.add_work_item("test").unwrap();
        assert_eq!(db.get_current_work(), Ok(None));
        let work_item = db.get_available_work().unwrap().first().unwrap().1;
        db.set_current_work(Some(work_item)).unwrap();
        t.advance(1);
        assert_eq!(db.get_current_work().unwrap(), Some(work_item));
    }

    #[test]
    fn shutdown() {
        let t = MockTime::new();
        let db = Database::open(":memory:", &t).unwrap();
        db.add_work_item("test").unwrap();
        let work_item = Some(db.get_available_work().unwrap().first().unwrap().1);
        db.set_current_work(work_item).unwrap();
        let start_time = t.now().with_timezone(&Local);
        t.advance(1);
        let end_time = t.now().with_timezone(&Local);
        db.shutdown().unwrap();
        db.add_work_end_at_shutdown().unwrap(); // Same day. Should do nothing
        let today = db.get_work_on_date(&t.now().date_naive()).unwrap();
        assert_eq!(today, vec![(work_item, start_time)]);
        t.advance(24);
        db.add_work_end_at_shutdown().unwrap(); // Next day. Should add None
        t.advance(-24);
        let today = db.get_work_on_date(&t.now().date_naive()).unwrap();
        assert_eq!(today, vec![(work_item, start_time), (None, end_time)]);
        t.advance(3);
        db.shutdown().unwrap();
        t.advance(24);
        db.add_work_end_at_shutdown().unwrap(); // Already None at day end. Should do nothing
        t.advance(-24);
        let today = db.get_work_on_date(&t.now().date_naive()).unwrap();
        assert_eq!(today, vec![(work_item, start_time), (None, end_time)]);
    }
}
