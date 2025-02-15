//use super::DBConfig;
use crate::test_disk::{BlockDevice, State};
/// Monitor in progress disk repairs
use chrono::offset::Utc;
use chrono::DateTime;
use helpers::{error::*, host_information::Host as MyHost, DBConfig};
use log::{debug, error, info};
use postgres::{params::ConnectParams, params::Host, rows::Row, transaction::Transaction};
use r2d2::{Pool, PooledConnection};
use r2d2_postgres::{PostgresConnectionManager as ConnectionManager, TlsMode};
use std::fmt::{Display, Formatter, Result as fResult};
use std::path::PathBuf;
use std::process::id;
use std::str::FromStr;
use std::time::Duration;

#[cfg(test)]
mod tests {
    use super::super::ConfigSettings;
    use block_utils::{Device, FilesystemType, MediaType, ScsiInfo};
    use simplelog::{Config, TermLogger};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use uuid::Uuid;

    #[test]
    fn test_new_host() {
        TermLogger::new(log::LevelFilter::Debug, Config::default()).unwrap();
        let info = super::MyHost::new().unwrap();
        println!("{:#?}", info);
    }

    #[test]
    fn test_db_apis() {
        TermLogger::new(log::LevelFilter::Debug, Config::default()).unwrap();
        let config_dir = Path::new("/newDevice/tests/");
        let config: ConfigSettings =
            helpers::load_config(config_dir, "bynar.json").expect("Failed to load config");
        let db_config = config.database;
        let pool = super::create_db_connection_pool(&db_config).unwrap();

        let info = super::MyHost::new().unwrap();
        let result = super::update_storage_info(&info, &pool).expect(
            "Failed to update
                storage details",
        );
        println!("Successfully updated storage details");
        println!("host details mapping {:#?}", result);

        // create fake device
        let drive_uuid = Uuid::new_v4();
        let dev_name = format!("some_path-{}", drive_uuid);
        let path = format!("/some/{}", dev_name);
        let mut d = crate::test_disk::BlockDevice {
            device: Device {
                id: Some(drive_uuid),
                name: dev_name,
                media_type: MediaType::Rotational,
                capacity: 26214400,
                fs_type: FilesystemType::Xfs,
                serial_number: Some("123456".into()),
            },
            dev_path: PathBuf::from(path),
            device_database_id: None,
            mount_point: None,
            partitions: BTreeMap::new(),
            scsi_info: ScsiInfo::default(),
            state: crate::test_disk::State::Unscanned,
            storage_detail_id: result.storage_detail_id,
            operation_id: None,
        };

        println!("Adding disk {:#?}", d);
        let _disk_result = super::add_disk_detail(&pool, &mut d).unwrap();
        let dev_id = match d.device_database_id {
            None => 0,
            Some(i) => i,
        };
        println!("Added disk with id {}", dev_id);

        // Add operation
        let mut op_info = super::OperationInfo::new(result.entry_id, dev_id);
        println!("Created operation {:#?}", op_info);
        let _op_result = super::add_or_update_operation(&pool, &mut op_info).unwrap();
        d.operation_id = op_info.operation_id;

        let o_id = match op_info.operation_id {
            None => 0,
            Some(i) => i,
        };
        println!("Added operation with ID {}", o_id);

        // call add_disk_detail again for same device
        println!(
            "Re-adding same disk with id {} again to the database",
            dev_id
        );
        let _disk_result2 = super::add_disk_detail(&pool, &mut d).unwrap();

        // Clear device_database_id to mimic re-run and add again
        d.device_database_id = None;

        let _disk_result3 = super::add_disk_detail(&pool, &mut d).unwrap();
        let new_dev_id = match d.device_database_id {
            None => 0,
            Some(i) => i,
        };
        println!(
            "Dev-id after reinsert attempt {}, old {}",
            new_dev_id, dev_id
        );

        // now update operation
        println!("Updating first operation with snapshot time");
        op_info.set_snapshot_time(super::Utc::now());
        let _op_result2 = super::add_or_update_operation(&pool, &mut op_info).unwrap();

        // update again with done_time
        op_info.set_done_time(super::Utc::now());
        println!("Updating first operation with done time");
        let _op_result2 = super::add_or_update_operation(&pool, &mut op_info).unwrap();

        // Add operation_details
        println!("Updating first operation detail as Evaluation");
        let mut op_detail = super::OperationDetail::new(o_id, super::OperationType::Evaluation);
        let _detail_result = super::add_or_update_operation_detail(&pool, &mut op_detail);
        // Update status
        println!("Updating first operation status as in-progress");
        op_detail.set_operation_status(super::OperationStatus::InProgress);
        let _detail_result = super::add_or_update_operation_detail(&pool, &mut op_detail);

        // Add another sub-operation
        println!("Updating first operation detail as WaitingForReplacement");
        let mut op_detail2 =
            super::OperationDetail::new(o_id, super::OperationType::WaitingForReplacement);
        op_detail2.set_operation_status(super::OperationStatus::InProgress);

        //update ticket_id
        println!("Updating second operation detail with tracking number");
        op_detail2.set_tracking_id("ABC-1234".to_string());
        let _detail_result = super::add_or_update_operation_detail(&pool, &mut op_detail2);

        // update first sub-operation as complete
        op_detail.set_operation_status(super::OperationStatus::Complete);
        op_detail.set_done_time(super::Utc::now());
        let _detail_result = super::add_or_update_operation_detail(&pool, &mut op_detail);

        // get device state from db
        let state = super::get_state(&pool, &d).unwrap();
        println!("State for dev name {} is {:#?}", d.device.name, state);

        let new_state = crate::test_disk::State::WaitingForReplacement;
        let _state_result = super::save_state(&pool, &d, new_state).unwrap();

        // get state again, and compare -- they should be same
        let new_state_result = super::get_state(&pool, &d).unwrap();
        println!(
            "State for dev name {} is {:#?}",
            d.device.name, new_state_result
        );
        assert_eq!(new_state, new_state_result);

        let tickets =
            super::get_outstanding_repair_tickets(&pool, result.storage_detail_id).unwrap();
        println!("All open tickets {:#?}", tickets);

        let is_repair_needed = super::is_hardware_waiting_repair(
            &pool,
            result.storage_detail_id,
            &d.device.name,
            None,
        )
        .unwrap();
        println!(
            "disk {} needs repair {}",
            d.dev_path.display(),
            is_repair_needed
        );

        let all_devices = super::get_devices_from_db(&pool, result.storage_detail_id).unwrap();
        println!("All devices {:#?}", all_devices);

        //TODO: add failure tests
        // 1. set entry_id = 0
    }
}

#[derive(Debug)]
pub struct DiskRepairTicket {
    pub ticket_id: String,
    pub device_name: String,
    pub device_path: String,
}

#[derive(Debug)]
pub struct DiskPendingTicket {
    pub ticket_id: String,
    pub device_name: String,
    pub device_path: String,
    pub device_id : i32,
}

impl DiskPendingTicket {
    pub fn new(ticket_id: String, device_name: String, device_path : String ,device_id : i32) -> DiskPendingTicket {
        DiskPendingTicket {
            ticket_id,
            device_name,
            device_path,
            device_id,
        }
    }
}

#[derive(Debug)]
pub struct HostDetailsMapping {
    pub entry_id: u32,
    pub region_id: u32,
    pub storage_detail_id: u32,
}

impl HostDetailsMapping {
    pub fn new(entry_id: u32, region_id: u32, storage_detail_id: u32) -> HostDetailsMapping {
        HostDetailsMapping {
            entry_id,
            region_id,
            storage_detail_id,
        }
    }
}

#[derive(Debug)]
pub struct OperationInfo {
    pub operation_id: Option<u32>,
    pub entry_id: u32,
    pub device_id: u32,
    pub behalf_of: Option<String>,
    pub reason: Option<String>,
    pub start_time: DateTime<Utc>,
    pub snapshot_time: DateTime<Utc>,
    pub done_time: Option<DateTime<Utc>>,
}

impl OperationInfo {
    pub fn new(entry_id: u32, device_id: u32) -> OperationInfo {
        OperationInfo {
            operation_id: None,
            entry_id,
            device_id,
            behalf_of: None,
            reason: None,
            start_time: Utc::now(),
            snapshot_time: Utc::now(),
            done_time: None,
        }
    }
    fn set_operation_id(&mut self, op_id: u32) {
        self.operation_id = Some(op_id);
    }
    pub fn set_done_time(&mut self, done_time: DateTime<Utc>) {
        self.done_time = Some(done_time);
    }
    pub fn set_snapshot_time(&mut self, snapshot_time: DateTime<Utc>) {
        self.snapshot_time = snapshot_time;
    }
}

#[derive(Debug)]
pub enum OperationType {
    DiskAdd,
    DiskReplace,
    DiskRemove,
    WaitingForReplacement,
    Evaluation,
}

impl Display for OperationType {
    fn fmt(&self, f: &mut Formatter<'_>) -> fResult {
        let message = match *self {
            OperationType::DiskAdd => "diskadd",
            OperationType::DiskReplace => "diskreplace",
            OperationType::DiskRemove => "diskremove",
            OperationType::WaitingForReplacement => "waiting_for_replacement",
            OperationType::Evaluation => "evaluation",
        };
        write!(f, "{}", message)
    }
}

#[derive(Debug)]
pub enum OperationStatus {
    Pending,
    InProgress,
    Complete,
}

impl Display for OperationStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fResult {
        let message = match *self {
            OperationStatus::Pending => "pending",
            OperationStatus::InProgress => "in_progress",
            OperationStatus::Complete => "complete",
        };
        write!(f, "{}", message)
    }
}

#[derive(Debug)]
pub struct OperationDetail {
    pub op_detail_id: Option<u32>,
    pub operation_id: u32,
    pub op_type: OperationType,
    pub status: OperationStatus,
    pub tracking_id: Option<String>,
    pub start_time: DateTime<Utc>,
    pub snapshot_time: DateTime<Utc>,
    pub done_time: Option<DateTime<Utc>>,
}

impl OperationDetail {
    pub fn new(operation_id: u32, op_type: OperationType) -> OperationDetail {
        OperationDetail {
            op_detail_id: None,
            operation_id,
            op_type,
            status: OperationStatus::Pending,
            tracking_id: None,
            start_time: Utc::now(),
            snapshot_time: Utc::now(),
            done_time: None,
        }
    }
    fn set_operation_detail_id(&mut self, op_detail_id: u32) {
        self.op_detail_id = Some(op_detail_id);
    }

    pub fn set_tracking_id(&mut self, tracking_id: String) {
        self.tracking_id = Some(tracking_id);
    }

    pub fn set_done_time(&mut self, done_time: DateTime<Utc>) {
        self.done_time = Some(done_time);
    }

    pub fn set_operation_status(&mut self, status: OperationStatus) {
        self.status = status;
    }
}

/// Reads the config file to establish a pool of database connections
pub fn create_db_connection_pool(db_config: &DBConfig) -> BynarResult<Pool<ConnectionManager>> {
    debug!(
        "Establishing a connection to database {} at {}:{} using {}",
        db_config.dbname, db_config.endpoint, db_config.port, db_config.username
    );
    // Postgres expects an &str here instead of Option<String>
    let password: Option<&str> = match db_config.password.as_ref() {
        Some(v) => Some(v),
        None => None,
    };
    let connection_params = ConnectParams::builder()
        .user(&db_config.username, password)
        .port(db_config.port)
        .database(&db_config.dbname)
        .build(Host::Tcp(db_config.endpoint.to_string()));
    let manager = ConnectionManager::new(connection_params, TlsMode::None)?;
    let db_pool = Pool::builder()
        .max_size(10)
        .connection_timeout(Duration::from_secs(300))
        .build(manager)?;
    Ok(db_pool)
}

/// return one connection from the pool
pub fn get_connection_from_pool(
    pool: &Pool<ConnectionManager>,
) -> BynarResult<PooledConnection<ConnectionManager>> {
    let connection = pool.get()?;
    Ok(connection)
}

/// Should be called when bynar daemon first starts up
/// Returns whether or not all steps in this call have been successful
/// TODO: return conn, entry_id, region_id, detail_id
pub fn update_storage_info(
    s_info: &MyHost,
    pool: &Pool<ConnectionManager>,
) -> BynarResult<HostDetailsMapping> {
    debug!("Adding datacenter and host information to database");

    // Get a database connection
    let conn = get_connection_from_pool(pool)?;
    // extract ip address to a &str
    let ip_address: String = s_info.ip.to_string();

    // Do all these three in a transaction, rolls back by default.
    let transaction = conn.transaction()?;
    info!("Started transaction to update storage information in database");
    let entry_id = register_to_process_manager(&transaction, &ip_address)?;
    let region_id = update_region(&transaction, &s_info.region)?;
    let detail_id = update_storage_details(&transaction, &s_info, region_id)?;

    let host_detail_mapping = if entry_id == 0 || region_id == 0 || detail_id == 0 {
        return Err(BynarError::new(
            "Failed to update storage information in the database".to_string(),
        ));
    } else {
        transaction.set_commit();
        HostDetailsMapping::new(entry_id, region_id, detail_id)
    };
    let _ = transaction.finish();
    Ok(host_detail_mapping)
}

/// responsible to store the pid, ip of the system on which bynar is running
fn register_to_process_manager(conn: &Transaction<'_>, ip: &str) -> BynarResult<u32> {
    // get process id
    let pid = id();
    debug!("Adding daemon details with pid {} to process manager", pid);
    let mut entry_id: u32 = 0;
    let stmt = format!(
        "SELECT entry_id FROM process_manager WHERE
    pid={} AND ip='{}'",
        pid, &ip
    );
    let stmt_query = conn.query(&stmt, &[])?;
    if let Some(row) = stmt_query.into_iter().next() {
        // entry exists for this ip with this pid. Update status
        let r: i32 = row.get("entry_id");
        let update_stmt = format!(
            "UPDATE process_manager SET status='idle'
           WHERE pid={} AND ip='{}'",
            pid, &ip
        );
        conn.execute(&update_stmt, &[])?;
        entry_id = r as u32;
    } else {
        // does not exist, insert
        let insert_stmt = format!(
            "INSERT INTO process_manager (pid, ip, status)
                            VALUES ({}, '{}', 'idle') RETURNING entry_id",
            pid, &ip
        );
        let insert_stmt_query = conn.query(&insert_stmt, &[])?;
        if let Some(r) = insert_stmt_query.into_iter().next() {
            let e: i32 = r.get("entry_id");
            entry_id = e as u32;
        }
    }
    Ok(entry_id)
}

/// Responsible to de-register itself when daemon exists
pub fn deregister_from_process_manager() -> BynarResult<()> {
    // DELETE FROM process_manager WHERE IP=<>
    Ok(())
}

// Checks for the region in the database, inserts if it does not exist
// and returns the region_id
fn update_region(conn: &Transaction<'_>, region: &str) -> BynarResult<u32> {
    let stmt = format!(
        "SELECT region_id FROM regions WHERE region_name = '{}'",
        region
    );
    let stmt_query = conn.query(&stmt, &[])?;
    let mut region_id: u32 = 0;

    if let Some(res) = stmt_query.into_iter().next() {
        // Exists, return region_id
        let id: i32 = res.get(0);
        region_id = id as u32;
    } else {
        // does not exist, insert
        debug!("Adding region {} to database", region);
        let stmt = format!(
            "INSERT INTO regions (region_name)
                            VALUES ('{}') RETURNING region_id",
            region
        );
        let stmt_query = conn.query(&stmt, &[])?;
        if let Some(res) = stmt_query.into_iter().next() {
            // Exists
            let id: i32 = res.get(0);
            region_id = id as u32;
        }
    }
    Ok(region_id)
}

fn update_storage_details(
    conn: &Transaction<'_>,
    s_info: &MyHost,
    region_id: u32,
) -> BynarResult<u32> {
    let stmt = format!(
        "SELECT storage_id FROM storage_types WHERE storage_type='{}'",
        s_info.storage_type
    );
    let stmt_query = conn.query(&stmt, &[])?;
    let mut storage_detail_id: u32 = 0;

    if let Some(r) = stmt_query.into_iter().next() {
        let sid: i32 = r.get("storage_id");
        let storage_id: u32 = sid as u32;

        // query if these storage details are already in DB
        let details_query = format!(
            "SELECT detail_id FROM storage_details WHERE storage_id = {}
            AND region_id = {} AND hostname = '{}'",
            storage_id, region_id, s_info.hostname
        );
        let details_query_exec = conn.query(&details_query, &[])?;
        if let Some(res) = details_query_exec.into_iter().next() {
            //Exists
            let sdi: i32 = res.get("detail_id");
            storage_detail_id = sdi as u32;
        } else {
            // TODO: modify when exact storage details are added

            let mut details_query = "INSERT INTO storage_details
            (storage_id, region_id, hostname"
                .to_string();
            if s_info.array_name.is_some() {
                details_query.push_str(", name_key1");
            }
            if s_info.pool_name.is_some() {
                details_query.push_str(", name_key2");
            }
            details_query.push_str(&format!(
                ") VALUES ({}, {}, '{}'",
                storage_id, region_id, s_info.hostname
            ));
            if let Some(ref array_name) = s_info.array_name {
                details_query.push_str(&format!(", '{}'", array_name));
            }
            if let Some(ref pool_name) = s_info.pool_name {
                details_query.push_str(&format!(", '{}'", pool_name));
            }
            details_query.push_str(") RETURNING detail_id");

            let dqr = conn.query(&details_query, &[])?;
            if let Some(res) = dqr.into_iter().next() {
                let sdi: i32 = res.get("detail_id");
                storage_detail_id = sdi as u32;
            } else {
                // failed to insert
                error!("Query to insert and retrive storage details failed");
            }
        }
    } else {
        error!("Storage type {} not in database", s_info.storage_type);
    }
    Ok(storage_detail_id)
}

// Inserts disk informatation record into bynar.hardware and adds the device_database_id to struct
pub fn add_disk_detail(
    pool: &Pool<ConnectionManager>,
    disk_info: &mut BlockDevice,
) -> BynarResult<()> {
    let conn = get_connection_from_pool(pool)?;
    let detail_id = disk_info.storage_detail_id as i32;

    let stmt_query = conn.query(
        "SELECT device_id FROM hardware WHERE device_path=$1
            AND detail_id=$2 AND device_name=$3",
        &[
            &format!("{}", disk_info.dev_path.display()),
            &detail_id,
            &disk_info.device.name,
        ],
    )?;
    if stmt_query.is_empty() {
        // A record doesn't exist, insert
        let mut stmt = String::new();

        let mut hardware_type: i32 = 2; // this is the usual value added to DB for disk type

        // Get hardware_type id from DB
        let stmt2 = conn.query(
            "SELECT hardware_id FROM hardware_types WHERE hardware_type='disk'",
            &[],
        )?;
        if let Some(res) = stmt2.into_iter().next() {
            hardware_type = res.get("hardware_id");
        }

        stmt.push_str(
            "INSERT INTO hardware(detail_id, device_path, device_name, state, hardware_type",
        );
        if disk_info.mount_point.is_some() {
            stmt.push_str(", mount_path");
        }
        if disk_info.device.id.is_some() {
            stmt.push_str(", device_uuid");
        }

        if disk_info.device.serial_number.is_some() {
            stmt.push_str(", serial_number");
        }

        stmt.push_str(&format!(
            ") VALUES ({}, '{}', '{}', '{}', {}",
            disk_info.storage_detail_id,
            disk_info.dev_path.display(),
            disk_info.device.name,
            disk_info.state,
            hardware_type
        ));

        if let Some(ref mount) = disk_info.mount_point {
            stmt.push_str(&format!(", '{}'", mount.display()));
        }
        if let Some(ref uuid) = disk_info.device.id {
            stmt.push_str(&format!(", '{}'", uuid));
        }
        if let Some(ref serial) = disk_info.device.serial_number {
            stmt.push_str(&format!(", '{}'", serial));
        }

        stmt.push_str(") RETURNING device_id");
        let stmt_q = conn.query(&stmt, &[])?;
        if let Some(row) = stmt_q.into_iter().next() {
            let id: i32 = row.get("device_id");
            disk_info.set_device_database_id(id as u32);
            Ok(())
        } else {
            Err(BynarError::new(format!(
                "Failed to add {},{} to database",
                disk_info.storage_detail_id, disk_info.device.name
            )))
        }
    } else {
        // device exists in database
        if let Some(result) = stmt_query.into_iter().next() {
            let id: i32 = result.get("device_id");
            // does it match our struct?
            match disk_info.device_database_id {
                None => {
                    disk_info.set_device_database_id(id as u32);
                    Ok(())
                }
                Some(i) => {
                    if i != id as u32 {
                        Err(BynarError::new(format!(
                            "Information about {} for storage id {} didn't match",
                            disk_info.device.name, disk_info.storage_detail_id
                        )))
                    } else {
                        Ok(())
                    }
                }
            }
        } else {
            // Query said something exists, but we couldn't find that
            Err(BynarError::new(format!(
                "Failed to find device information for {},{} in database",
                disk_info.storage_detail_id, disk_info.device.name
            )))
        }
    }
}

// inserts the operation record. If successful insert, the provided input op_info
// is modified. Returns error if insert or update fails.
pub fn add_or_update_operation(
    pool: &Pool<ConnectionManager>,
    op_info: &mut OperationInfo,
) -> BynarResult<()> {
    let mut stmt = String::new();

    let conn = get_connection_from_pool(pool)?;
    match op_info.operation_id {
        None => {
            // no operation_id, validate new record input
            if op_info.entry_id == 0 {
                return Err(BynarError::new(
                    "A process tracking ID is required and is missing".to_string(),
                ));
            }
            stmt.push_str(
                "INSERT INTO operations (
                                    entry_id, start_time, snapshot_time, device_id",
            );

            if op_info.behalf_of.is_some() {
                stmt.push_str(", behalf_of");
            }
            if op_info.reason.is_some() {
                stmt.push_str(", reason");
            }

            stmt.push_str(")");

            stmt.push_str(&format!(
                " VALUES ({},'{}', '{}', {}",
                op_info.entry_id, op_info.start_time, op_info.snapshot_time, op_info.device_id
            ));

            if let Some(ref behalf_of) = op_info.behalf_of {
                stmt.push_str(&format!(", '{}'", behalf_of));
            }
            if let Some(ref reason) = op_info.reason {
                stmt.push_str(&format!(", '{}'", reason));
            }
            stmt.push_str(") RETURNING operation_id");
        }
        Some(id) => {
            // update existing record. Only snapshot_time and done_time
            // can be updated.
            stmt.push_str(&format!(
                "UPDATE operations SET snapshot_time = '{}'",
                op_info.snapshot_time
            ));

            if let Some(d_time) = op_info.done_time {
                stmt.push_str(&format!(", done_time = '{}'", d_time));
            }
            stmt.push_str(&format!(" WHERE operation_id = {}", id));
        }
    }
    let stmt_query = conn.query(&stmt, &[])?;
    match op_info.operation_id {
        None => {
            // insert
            if let Some(row) = stmt_query.into_iter().next() {
                let oid: i32 = row.get("operation_id");
                op_info.set_operation_id(oid as u32);
                Ok(())
            } else {
                Err(BynarError::new(
                    "Query to insert operation into DB failed".to_string(),
                ))
            }
        }
        Some(_) => {
            // update. even if query to update failed that's fine.
            Ok(())
        }
    }
}

pub fn add_or_update_operation_detail(
    pool: &Pool<ConnectionManager>,
    operation_detail: &mut OperationDetail,
) -> BynarResult<()> {
    let conn = get_connection_from_pool(pool)?;
    let mut stmt = String::new();
    match operation_detail.op_detail_id {
        None => {
            // insert new detail record
            let stmt2 = format!(
                "SELECT type_id FROM operation_types WHERE
                                op_name='{}'",
                operation_detail.op_type
            );
            let stmt_query = conn.query(&stmt2, &[])?;
            if stmt_query.len() != 1 {
                return Err(BynarError::new(format!(
                    "More than one record found in database for operation {}",
                    operation_detail.op_type
                )));
            }
            if stmt_query.is_empty() {
                return Err(BynarError::new(format!(
                    "No record in database for operation {}",
                    operation_detail.op_type
                )));
            }
            let row = stmt_query.get(0);
            let type_id: i32 = row.get("type_id");

            stmt.push_str(
                "INSERT INTO operation_details (operation_id, type_id,
                            status, start_time, snapshot_time",
            );
            if operation_detail.tracking_id.is_some() {
                stmt.push_str(", tracking_id");
            }
            if operation_detail.done_time.is_some() {
                stmt.push_str(", done_time");
            }

            stmt.push_str(&format!(
                " ) VALUES ({}, {}, '{}', '{}', '{}'",
                operation_detail.operation_id,
                type_id as u32,
                operation_detail.status,
                operation_detail.start_time,
                operation_detail.snapshot_time
            ));

            if let Some(ref t_id) = operation_detail.tracking_id {
                stmt.push_str(&format!(", '{}'", t_id));
            }
            if let Some(done_time) = operation_detail.done_time {
                stmt.push_str(&format!(", '{}'", done_time));
            }
            stmt.push_str(") RETURNING operation_detail_id");
        }
        Some(id) => {
            // update existing detail record.
            // Only tracking_id, snapshot_time, done_time and status are update-able
            stmt.push_str(&format!(
                "UPDATE operation_details SET snapshot_time = '{}', 
                            status = '{}'",
                operation_detail.snapshot_time, operation_detail.status
            ));
            if let Some(ref t_id) = operation_detail.tracking_id {
                stmt.push_str(&format!(", tracking_id = '{}'", t_id));
            }
            if let Some(done_time) = operation_detail.done_time {
                stmt.push_str(&format!(", done_time = '{}'", done_time));
            }
            stmt.push_str(&format!(" WHERE operation_detail_id = {}", id));
        }
    }

    let stmt_query = conn.query(&stmt, &[])?;
    if operation_detail.op_detail_id.is_none() {
        // insert.
        if let Some(row) = stmt_query.into_iter().next() {
            let oid: i32 = row.get("operation_detail_id");
            operation_detail.set_operation_detail_id(oid as u32);
        } else {
            return Err(BynarError::new(
                "Query to insert operation detail into database failed".to_string(),
            ));
        }
    }
    // update. even if query to update failed that's fine.
    Ok(())
}

pub fn save_state(
    pool: &Pool<ConnectionManager>,
    device_detail: &BlockDevice,
    state: State,
) -> BynarResult<()> {
    debug!(
        "Saving state as {} for device {}",
        state, device_detail.device.name
    );
    let conn = get_connection_from_pool(pool)?;

    if let Some(dev_id) = device_detail.device_database_id {
        // Device is in database, update the state. Start a transaction to roll back if needed.
        // transaction rolls back by default.
        let transaction = conn.transaction()?;
        let stmt = format!(
            "UPDATE hardware SET state = '{}' WHERE device_id={}",
            state, dev_id
        );
        let stmt_query = transaction.execute(&stmt, &[])?;
        info!(
            "Updated {} rows in database with state information",
            stmt_query
        );
        if stmt_query != 1 {
            // Only one device should  be updated. Rollback
            transaction.set_rollback();
            let _ = transaction.finish();
            Err(BynarError::new(
                "Attempt to update more than one device in database. Rolling back.".to_string(),
            ))
        } else {
            transaction.set_commit();
            let _ = transaction.finish();
            Ok(())
        }
    } else {
        // device is not in database. It should have been.
        Err(BynarError::new(format!(
            "Device {} for storage detail with id {} is not in database",
            device_detail.device.name, device_detail.storage_detail_id
        )))
    }
}

pub fn save_smart_result(
    pool: &Pool<ConnectionManager>,
    device_detail: &BlockDevice,
    smart_passed: bool,
) -> BynarResult<()> {
    debug!(
        "Saving smart check result as {} for device {}",
        smart_passed, device_detail.device.name
    );
    let conn = get_connection_from_pool(pool)?;

    if let Some(dev_id) = device_detail.device_database_id {
        // Device is in database, update smart_passed. Start a transaction to roll back if needed.
        // transaction rolls back by default.
        let transaction = conn.transaction()?;
        let stmt = format!(
            "UPDATE hardware SET smart_passed = {} WHERE device_id={}",
            smart_passed, dev_id
        );
        let stmt_query = transaction.execute(&stmt, &[])?;
        info!(
            "Updated {} rows in database with smart check result",
            stmt_query
        );
        if stmt_query != 1 {
            // Only one device should  be updated. Rollback
            transaction.set_rollback();
            transaction.finish()?;
            Err(BynarError::new(
                "Attempt to update more than one device in database. Rolling back.".to_string(),
            ))
        } else {
            transaction.set_commit();
            transaction.finish()?;
            Ok(())
        }
    } else {
        // device is not in database. It should have been.
        Err(BynarError::new(format!(
            "Device {} for storage detail with id {} is not in database",
            device_detail.device.name, device_detail.storage_detail_id
        )))
    }
}

// Returns the currently known disks from the database.
pub fn get_devices_from_db(
    pool: &Pool<ConnectionManager>,
    storage_detail_id: u32,
) -> BynarResult<Vec<(u32, String, PathBuf)>> {
    debug!("Retrieving devices from DB",);
    let conn = get_connection_from_pool(pool)?;

    let detail_id = storage_detail_id as i32;
    let stmt_query = conn.query(
        "select device_id, device_name, device_path from hardware where detail_id=$1 AND hardware_type=(SELECT hardware_id FROM hardware_types WHERE hardware_type='disk')",
        &[&detail_id],
    )?;

    let mut devices: Vec<(u32, String, PathBuf)> = Vec::new();
    for row in stmt_query.iter() {
        let dev_id: i32 = row.get(0);
        let dev_name: String = row.get(1);
        let dev_path: String = row.get(2);
        devices.push((dev_id as u32, dev_name, PathBuf::from(dev_path)));
    }
    Ok(devices)
}

/// Returns the state information from the database.
/// Returns error if no record of device is found in the database.
/// Returns the default state if state was not previously saved.
pub fn get_state(
    pool: &Pool<ConnectionManager>,
    device_detail: &BlockDevice,
) -> BynarResult<State> {
    debug!(
        "Retrieving state for device {} with storage detail id {} from DB",
        device_detail.device.name, device_detail.storage_detail_id
    );
    let conn = get_connection_from_pool(pool)?;

    match device_detail.device_database_id {
        Some(dev_id) => {
            let dev_id = dev_id as i32;
            let stmt_query = conn.query(
                "SELECT state FROM hardware WHERE device_id = $1",
                &[&dev_id],
            )?;
            if stmt_query.len() != 1 || stmt_query.is_empty() {
                // Database doesn't know about the device.  Must be new disk.
                Ok(State::Unscanned)
            } else {
                let row = stmt_query.get(0);
                let retrieved_state: String = row.get("state");
                Ok(State::from_str(&retrieved_state).unwrap_or(State::Unscanned))
            }
        }
        None => {
            // No entry of this device in database table. Cannot get state information
            Err(BynarError::new(format!(
                "Device {} for storage detail {} is not in DB",
                device_detail.device.name, device_detail.storage_detail_id
            )))
        }
    }
}

/// Returns whether smart checks have passed information from the database.
/// Returns error if no record of device is found in the database.
/// Returns false if not previously saved.
pub fn get_smart_result(
    pool: &Pool<ConnectionManager>,
    device_detail: &BlockDevice,
) -> BynarResult<bool> {
    debug!(
        "Retrieving smart check result for device {} with storage detail id {} from DB",
        device_detail.device.name, device_detail.storage_detail_id
    );
    let conn = get_connection_from_pool(pool)?;

    if let Some(dev_id) = device_detail.device_database_id {
        let stmt = format!(
            "SELECT smart_passed FROM hardware WHERE device_id = {}",
            dev_id
        );
        let stmt_query = conn.query(&stmt, &[])?;
        if stmt_query.len() != 1 || stmt_query.is_empty() {
            // Query didn't return anything. Assume smart checks have not been done/passed
            Ok(false)
        } else {
            // got something from the database
            let row = stmt_query.get(0);
            let smart_passed = row.get("smart_passed");
            Ok(smart_passed)
        }
    } else {
        // No entry of this device in database table. Cannot get smart_cheks info
        Err(BynarError::new(format!(
            "Device {} for storage detail {} is not in DB",
            device_detail.device.name, device_detail.storage_detail_id
        )))
    }
}

fn row_to_ticket(row: &Row<'_>) -> DiskRepairTicket {
    DiskRepairTicket {
        ticket_id: row.get(0),
        device_name: row.get(1),
        device_path: row.get(2),
    }
}

/// Get a list of ticket IDs (JIRA/other ids) that belong to me.
/// that are pending in op_type=waitForReplacement
pub fn get_outstanding_repair_tickets(
    pool: &Pool<ConnectionManager>,
    storage_detail_id: u32,
) -> BynarResult<Vec<DiskRepairTicket>> {
    let conn = get_connection_from_pool(pool)?;

    // Get all tickets of myself with device.state=WaitingForReplacement and operation_detail.status = pending or in_progress
    let stmt = "SELECT tracking_id, device_name, device_path FROM operation_details JOIN operations USING (operation_id)
     JOIN hardware USING (device_id) WHERE 
     (status=$1 OR status=$2) AND 
     type_id = (SELECT type_id FROM operation_types WHERE op_name= $3) AND 
     hardware.state in ($4, $5) AND 
     detail_id = $6 AND  
     tracking_id IS NOT NULL ORDER BY operations.start_time";

    let detail_id = storage_detail_id as i32;
    let stmt_query = conn.query(
        &stmt,
        &[
            &OperationStatus::InProgress.to_string(),
            &OperationStatus::Pending.to_string(),
            &OperationType::WaitingForReplacement.to_string(),
            &State::WaitingForReplacement.to_string(),
            &State::Good.to_string(),
            &detail_id,
        ],
    )?;
    let mut tickets: Vec<DiskRepairTicket> = Vec::new();
    if stmt_query.is_empty() {
        debug!(
            "No pending or in-progress tickets for this host with detail id {}",
            storage_detail_id
        );
        Ok(tickets)
    } else {
        debug!(
            "{} pending tickets for this host with detail id {}",
            stmt_query.len(),
            storage_detail_id
        );
        for row in stmt_query.iter() {
            // TODO [SD]: use postgres_derive
            tickets.push(row_to_ticket(&row));
        }
        Ok(tickets)
    }
}

/// Sets status=Complete for the record that has the given ticket_id.
/// Equivalent to calling add_or_update_operation_detail() with appropriate fields set
pub fn resolve_ticket_in_db(pool: &Pool<ConnectionManager>, ticket_id: &str) -> BynarResult<()> {
    let conn = get_connection_from_pool(pool)?;
    debug!("Attempting to resolve ticket {}", ticket_id);

    // TODO[SD]: make sure there is one ticket with this ID
    let stmt = format!(
        "UPDATE operation_details SET status='{}' WHERE ticket_id='{}'",
        OperationStatus::Complete,
        ticket_id
    );
    let stmt_query = conn.execute(&stmt, &[])?;
    info!(
        "Updated {} rows in database. Ticket {} marked as complete.",
        stmt_query, ticket_id
    );
    Ok(())
}

pub fn is_hardware_waiting_repair(
    pool: &Pool<ConnectionManager>,
    storage_detail_id: u32,
    device_name: &str,
    serial_number: Option<&str>,
) -> BynarResult<bool> {
    let conn = get_connection_from_pool(pool)?;
    // is there is any operation for this hardware that is waiting for replacement
    let mut stmt = "SELECT status FROM operation_details 
    JOIN operations USING (operation_id) 
    JOIN hardware USING (device_id) 
    WHERE device_name=$1 AND 
    detail_id=$2 AND 
    type_id = (SELECT type_id FROM operation_types WHERE op_name=$3) AND 
    state=$4"
        .to_string();
    let detail_id = storage_detail_id as i32;
    let operation_type = OperationType::WaitingForReplacement.to_string();
    let state_type = State::WaitingForReplacement.to_string();
    let mut params: Vec<&postgres::types::ToSql> =
        vec![&device_name, &detail_id, &operation_type, &state_type];
    // Add the serial_number to the query if given
    if let Some(ref serial) = serial_number {
        stmt.push_str(" AND device_uuid=$5");
        params.push(serial);
    }

    let stmt_query = conn.query(&stmt, &params)?;
    Ok(!stmt_query.is_empty())
}

/// Get region id based on the region name.
pub fn get_region_id(pool: &Pool<ConnectionManager>, region_name: &str) -> BynarResult<Option<u32>> {
    let conn = get_connection_from_pool(pool)?;

    // Get region Id from region name
    let stmt = "SELECT region_id FROM regions WHERE region_name = $1";
    let stmt_query = conn.query(stmt, &[&region_name])?;

    if let Some(res) = stmt_query.into_iter().next() {
        // Exists, return region_id
        let id: i32 = res.get(0);
        debug!("Region id {} for the region {}", id, region_name);
        Ok(Some(id as u32))
    } else {
        // does not exist
        debug!("No region with name {} in database", region_name);
        Ok(None)
    }
    
}

/// Get storage id based on the storage type.
pub fn get_storage_id(pool: &Pool<ConnectionManager>, storage_type: &str) -> BynarResult<Option<u32>> {
    let conn = get_connection_from_pool(pool)?;

    // Get storage Id from storage type
    let stmt = "SELECT storage_id FROM storage_types WHERE storage_type= $1 ";
    let stmt_query = conn.query(&stmt, &[&storage_type])?;

    if let Some(res) = stmt_query.into_iter().next() {
        // Exists, return storage_id
        let id: i32 = res.get(0);
        debug!(
            "Storage id {} for the storage_type {}",
            id, storage_type
        );
        Ok(Some(id as u32))
    } else {
        // does not exist
        debug!("No storage with type {} in database", storage_type);
        Ok(None)
    }
}

/// Get storage detail id based on the storage id, region id and hotsname
pub fn get_storage_detail_id(
    pool: &Pool<ConnectionManager>,
    storage_id: u32,
    region_id: u32,
    host_name: &str,
) -> BynarResult<Option<u32>> {
    let conn = get_connection_from_pool(pool)?;

    // Get storage detail Id
    let stmt = "SELECT detail_id FROM storage_details WHERE storage_id = $1
            AND region_id = $2 AND hostname = $3 ";
    let stmt_query = conn.query(&stmt, &[&storage_id,&region_id, &host_name])?;

    if let Some(res) = stmt_query.into_iter().next() {
        // Exists, return storage_id
        let id: i32 = res.get(0);
        debug!(
            "Storage details id {} for the host_name {} , region {} , storage_id {} ",
            id, host_name, region_id, storage_id
        );
        Ok(Some(id as u32))
    } else {
        // does not exist
        debug!(
            "No storage detail id with host_name {} , region {} , storage_id {} 
        in database",
            host_name, region_id, storage_id,
        );
        Ok(None)
    }
}

/// Get a list of ticket IDs (JIRA/other ids) that belong to all servers.
/// that are in pending state  and outstanding tickets
pub fn get_all_pending_tickets(
    pool: &Pool<ConnectionManager>
) -> BynarResult<Vec<DiskPendingTicket>> {
    let conn = get_connection_from_pool(pool)?;

    // Get all tickets with device.state=WaitingForReplacement and operation_detail.status = pending or in_progress
     let stmt = "SELECT tracking_id, device_name, device_path, device_id FROM operation_details JOIN operations
     USING (operation_id) JOIN hardware USING (device_id) WHERE
     (status=$1 OR status=$2) AND
     type_id = (SELECT type_id FROM operation_types WHERE op_name= $3) AND
     hardware.state in ($4, $5) AND tracking_id IS NOT NULL ORDER BY operations.start_time";


    let stmt_query = conn.query(
        &stmt,
        &[
            &OperationStatus::InProgress.to_string(),
            &OperationStatus::Pending.to_string(),
            &OperationType::WaitingForReplacement.to_string(),
            &State::WaitingForReplacement.to_string(),
            &State::Good.to_string()
        ],
    )?;
    
    if stmt_query.is_empty() {
        debug!(
            "No pending tickets for any host "
        );
        Ok(vec![])
    } else {
        let mut tickets: Vec<DiskPendingTicket> = Vec::with_capacity(stmt_query.len());
        debug!(
            "{} pending tickets for all hosts ",
            stmt_query.len()
        );
        for row in stmt_query.iter() {
            tickets.push(DiskPendingTicket::new(row.get(0),row.get(1),row.get(2),row.get(3)));
        }
        Ok(tickets)
    }
}

/// Get host name based on the device id 
pub fn get_host_name(
    pool: &Pool<ConnectionManager>,
    device_id: i32,
) -> BynarResult<Option<String>> {
    let conn = get_connection_from_pool(pool)?;

    // Get host name
    let stmt = "SELECT hostname FROM storage_details JOIN hardware USING (detail_id) WHERE device_id = $1; ";
    let stmt_query = conn.query(&stmt, &[&device_id])?;

    if let Some(res) = stmt_query.into_iter().next() {
        // Exists, return host name
        let host_name: String = res.get("hostname");
        debug!(
            "host_name {} for device_id {} ",
            host_name, device_id
        );
        Ok(Some(host_name))
    } else {
        // does not exist
        debug!(
            "No host_name for device_id {} in database",
             device_id,
        );
        Ok(None)
    }
}
