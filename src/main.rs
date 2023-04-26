#![windows_subsystem = "windows"]
use ::std::rc::Rc;
use chrono::Utc;
use rusqlite::OptionalExtension;

mod business_logic;
mod database;

use database::Database;

use gtk::{glib, Application};
use gtk::{prelude::*, ApplicationWindow, CheckButton};

const APP_ID: &str = "org.pathim.Timetrax";

fn main() -> glib::ExitCode {
    let db = Rc::new(Database::open("work.db", &Utc).expect("Unable to open database"));
    // Create a new application
    let app = Application::builder().application_id(APP_ID).build();

    let db2 = db.clone();
    app.connect_activate(move |a| (build_ui(a, &db2)));

    // Run the application
    app.run()
}

fn build_ui(app: &Application, db: &Database<Utc>) {
    let items = db.get_available_work().expect("No work available");
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let window = ApplicationWindow::builder()
        .application(app)
        .title("Timetrax")
        .child(&vbox)
        .build();
    let mut prev = None;
    for (name, id) in items {
        let btn = CheckButton::builder().label(name).build();
        btn.set_group(prev.as_ref());
        if prev.is_none() {
            btn.set_active(true);
        }
        vbox.append(&btn);
        prev = Some(btn);
    }
    window.present();
}
