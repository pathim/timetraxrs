use chrono::{DateTime, Local};
use rusqlite::{Connection, OptionalExtension, Result};

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open() -> Result<Self> {
        let conn = Connection::open("work.db")?;
        let s = Database { conn };
        s.conn.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_FKEY,
            true,
        )?;
        s.conn.execute("CREATE TABLE IF NOT EXISTS work_items (id INTEGER PRIMARY KEY ASC, name TEXT NOT NULL UNIQUE, description TEXT, visible BOOLEAN NOT NULL);", ())?;
        s.conn.execute("CREATE TABLE IF NOT EXISTS work_times (start TEXT NOT NULL UNIQUE, work_item INTEGER, FOREIGN KEY (work_item) REFERENCES work_items (id));", ())?;
        s.conn.execute(
            "CREATE TABLE IF NOT EXISTS key_value (key TEXT PRIMARY KEY, value TEXT);",
            (),
        )?;

        s.create_default_items()?;

        // Check if time of last shutdown was yesterday or earlier. Then add shutdown time as end of workday if no end was inserted before
        let last_shutdown: Option<String> = s
            .conn
            .query_row(
                "SELECT value FROM key_value WHERE key='shutdown' AND date('now','localtime')>date(value,'localtime');",
                (),
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        if let Some(shutdown_time) = last_shutdown {
            let last_work:Option<u64>=s.conn.query_row("SELECT work_item FROM work_times WHERE date(start,'localtime')=date(?,'localtime') ORDER BY start DESC LIMIT 1", [&shutdown_time], |row| row.get(0)).optional().unwrap().flatten();
            if last_work.is_some() {
                s.conn.execute(
                    "INSERT INTO work_times (start,work_item) VALUES (?,NULL)",
                    [&shutdown_time],
                )?;
            }
        }

        Ok(s)
    }

    pub fn create_default_items(&self) -> Result<()> {
        self.conn.execute("INSERT OR IGNORE INTO work_items(name, description, visible) VALUES ('Standup',NULL,1);", ())?;
        Ok(())
    }
    pub fn add_work_item(&self, name: &str) -> Result<usize> {
        self.conn.execute(
            "INSERT OR IGNORE INTO work_items(name, description, visible) VALUES (?,NULL,1);",
            [name],
        )
    }
    pub fn shutdown(&self) -> Result<()> {
        self.conn.execute("INSERT INTO key_value(key, value) VALUES ('shutdown', datetime('now')) ON CONFLICT DO UPDATE SET value=excluded.value;", ())?;
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
        self.conn.query_row("SELECT work_item FROM work_times WHERE date(start,'localtime')=date('now','localtime') ORDER BY start DESC LIMIT 1", (), |row| row.get(0)).optional().map(|x| x.flatten())
    }
    pub fn set_current_work(&self, work_item: Option<u64>) -> Result<()> {
        self.conn.execute("INSERT INTO work_times (start,work_item) VALUES (datetime('now'),?) ON CONFLICT DO UPDATE SET work_item=excluded.work_item;", [work_item])?;
        Ok(())
    }
    pub fn get_work_today(&self) -> Result<Vec<(Option<u64>, DateTime<Local>)>> {
        let mut stmt=self.conn.prepare("SELECT work_item,start FROM work_times WHERE date(start,'localtime')=date('now','localtime') ORDER BY start ASC;")?;
        let res = stmt.query_map((), |row| Ok((row.get(0)?, row.get(1)?)))?;
        res.collect()
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        self.shutdown().ok();
    }
}
