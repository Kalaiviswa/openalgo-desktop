//! What the runner page reads and changes about deployments apart from
//! running them (web `blueprints/openscript_runner.py`): the saved run
//! settings and the schedules. Starting, pausing, closing and removing a
//! deployment are the registry's (`mod.rs`).

use super::store::{self, Deployment, Schedule, SettingsInput};
use super::{Refusal, Runner};
use std::collections::BTreeMap;

/// A saved deployment: its key, the message for the trader and the
/// settings as now stored.
pub type Saved = (String, String, Option<Deployment>);

impl Runner {
    /// Every schedule and every deployment's saved settings. What cannot be
    /// read reads as none.
    pub fn saved_overview(&self) -> (BTreeMap<String, Schedule>, BTreeMap<String, Deployment>) {
        match self.db.conn() {
            Ok(conn) => (
                store::all_schedules(&conn).unwrap_or_default(),
                store::all_deployments(&conn).unwrap_or_default(),
            ),
            Err(_) => Default::default(),
        }
    }

    /// The schedule and the saved settings of `name`. What cannot be read
    /// reads as none.
    pub fn saved_of(&self, name: &str) -> (Option<Schedule>, Option<Deployment>) {
        match self.db.conn() {
            Ok(conn) => (
                store::all_schedules(&conn)
                    .ok()
                    .and_then(|mut s| s.remove(name)),
                store::read(&conn, name).ok().flatten(),
            ),
            Err(_) => (None, None),
        }
    }

    /// Every deployment's saved settings; none when they cannot be read.
    pub fn saved_settings(&self) -> BTreeMap<String, Deployment> {
        self.db
            .conn()
            .ok()
            .and_then(|c| store::all_deployments(&c).ok())
            .unwrap_or_default()
    }

    /// The saved settings of `name`; refused with why there are none (404),
    /// or 500 when the database cannot be read.
    pub fn settings_of(&self, name: &str) -> Result<Deployment, Refusal> {
        let Ok(conn) = self.db.conn() else {
            return Err(Refusal::new(
                500,
                "The run settings could not be read just now.",
            ));
        };
        match store::read(&conn, name) {
            Ok(Some(d)) => Ok(d),
            _ => {
                let why = match store::require(&conn, name) {
                    Ok(Err(w)) => w,
                    _ => format!("{} has no run settings saved.", name),
                };
                Err(Refusal::new(404, why))
            }
        }
    }

    /// Save the run settings `input` for `name`.
    pub fn save_settings(&self, name: &str, input: SettingsInput) -> Result<Saved, Refusal> {
        const NOT_SAVED: &str = "These run settings could not be saved on this server";
        let Ok(mut conn) = self.db.conn() else {
            return Err(Refusal::new(500, NOT_SAVED));
        };
        match store::write(&mut conn, name, input, self.clock.now()) {
            Ok(Ok((key, message))) => {
                let saved = store::read(&conn, &key).ok().flatten();
                Ok((key, message, saved))
            }
            Ok(Err(why)) => Err(Refusal::new(400, why)),
            Err(e) => {
                tracing::error!("Could not save OpenScript run settings: {}", e);
                Err(Refusal::new(400, NOT_SAVED))
            }
        }
    }

    /// Store `name`'s schedule. `Err` is the cause, for the log.
    pub fn store_schedule(&self, name: &str, entry: &Schedule) -> Result<(), String> {
        let conn = self.db.conn().map_err(|e| e.to_string())?;
        store::set_schedule(&conn, name, entry, self.clock.now()).map_err(|e| e.to_string())
    }

    /// Remove `name`'s schedule. `Err` is the cause, for the log.
    pub fn remove_schedule(&self, name: &str) -> Result<(), String> {
        let conn = self.db.conn().map_err(|e| e.to_string())?;
        store::delete_schedule(&conn, name)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}
