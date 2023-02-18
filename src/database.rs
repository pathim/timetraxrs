use std::path::Path;

use chrono::{DateTime, Duration, Local};
use rusqlite::{Connection, OptionalExtension, Result};

pub struct Database {
    conn: Connection,
    now: fn() -> DateTime<chrono::Utc>,
}

impl Database {
    pub fn open<P: AsRef<Path>>(
        path: P,
        now_provider: fn() -> DateTime<chrono::Utc>,
    ) -> Result<Self> {
        let conn = Connection::open(path)?;
        let s = Database {
            conn,
            now: now_provider,
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
        let last_shutdown: Option<String> = self.conn.query_row("SELECT value FROM key_value WHERE key='shutdown' AND date(?,'localtime')>date(value,'localtime');",((self.now)(),),|row| row.get(0),).optional().unwrap();
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
            ((self.now)(), &default_time),
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
        self.conn.execute("INSERT INTO key_value(key, value) VALUES ('shutdown', ?) ON CONFLICT DO UPDATE SET value=excluded.value;", ((self.now)(),))?;
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
        self.conn.query_row("SELECT work_item FROM work_times WHERE date(start,'localtime')=date(?,'localtime') ORDER BY start DESC LIMIT 1", ((self.now)(),), |row| row.get(0)).optional().map(|x| x.flatten())
    }
    pub fn set_current_work(&self, work_item: Option<u64>) -> Result<()> {
        self.conn.execute("INSERT INTO work_times (start,work_item) VALUES (?,?) ON CONFLICT DO UPDATE SET work_item=excluded.work_item;", ((self.now)(),work_item))?;
        Ok(())
    }
    pub fn get_work_today(&self) -> Result<Vec<(Option<u64>, DateTime<Local>)>> {
        let mut stmt=self.conn.prepare("SELECT work_item,start FROM work_times WHERE date(start,'localtime')=date(?,'localtime') ORDER BY start ASC;")?;
        let res = stmt.query_map(((self.now)(),), |row| Ok((row.get(0)?, row.get(1)?)))?;
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
            ((self.now)(),),
            |row| row.get(0),
        )?;
        let expected = Duration::seconds(expected);
        let mut stmt=self.conn.prepare("SELECT work_item,start FROM work_times WHERE date(start,'localtime')<date(?,'localtime') ORDER BY start ASC;")?;
        let res = stmt.query_map(((self.now)(),), |row| Ok((row.get(0)?, row.get(1)?)))?;
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
                ((self.now)(),),
                |row| row.get(0),
            )
            .map(Duration::seconds)
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        self.shutdown().ok();
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use std::collections::HashSet;

    use super::Database;
    #[test]
    fn add_get_work_item() {
        let db =
            Database::open(":memory:", || chrono::Utc.timestamp_millis_opt(0).unwrap()).unwrap();
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
}
