use crate::job::{Job, JobToRunAsync};
use crate::job_data::{JobState, JobStoredData, JobType};
use crate::job_store::JobStoreLocked;
use crate::{JobScheduler, JobSchedulerError, JobToRun, OnJobNotification};
use chrono::{DateTime, Utc};
use cron::Schedule;
use std::ops::Add;
use std::sync::mpsc::Receiver;
use std::time::{Duration, SystemTime};
use tokio::task::JoinHandle;
use uuid::Uuid;

pub struct NonCronJob {
    pub run: Box<JobToRun>,
    pub run_async: Box<JobToRunAsync>,
    pub join_handle: Option<JoinHandle<()>>,
    pub data: JobStoredData,
    pub async_job: bool,
}

impl Job for NonCronJob {
    fn is_cron_job(&self) -> bool {
        false
    }

    fn schedule(&self) -> Option<Schedule> {
        None
    }

    fn last_tick(&self) -> Option<DateTime<Utc>> {
        self.data
            .last_tick
            .map(|lt| SystemTime::UNIX_EPOCH.add(Duration::from_secs(lt)))
            .map(DateTime::from)
    }

    fn set_last_tick(&mut self, tick: Option<DateTime<Utc>>) {
        self.data.last_tick = tick.map(|t| t.timestamp() as u64);
    }

    fn set_count(&mut self, count: u32) {
        self.data.count = count;
    }

    fn count(&self) -> u32 {
        self.data.count
    }

    fn increment_count(&mut self) {
        self.data.count = if self.data.count + 1 < u32::MAX {
            self.data.count + 1
        } else {
            0
        }; // Overflow check
    }

    fn job_id(&self) -> Uuid {
        self.data.id.as_ref().cloned().map(|e| e.into()).unwrap()
    }

    fn run(&mut self, jobs: JobScheduler) -> Receiver<bool> {
        let (tx, rx) = std::sync::mpsc::channel();
        let job_id = self.job_id();

        if !self.async_job {
            (self.run)(job_id, jobs);
            if let Err(e) = tx.send(true) {
                eprintln!("Error notifying done {:?}", e);
            }
        } else {
            let future = (self.run_async)(job_id, jobs);
            tokio::task::spawn(async move {
                future.await;
                if let Err(e) = tx.send(true) {
                    eprintln!("Error notifying done {:?}", e);
                }
            });
        }
        rx
    }

    fn job_type(&self) -> JobType {
        self.data.job_type()
    }

    fn ran(&self) -> bool {
        self.data.ran
    }

    fn set_ran(&mut self, ran: bool) {
        self.data.ran = ran;
    }

    fn set_join_handle(&mut self, handle: Option<JoinHandle<()>>) {
        self.join_handle = handle;
    }

    fn stop(&self) -> bool {
        self.data.stopped
    }

    fn set_stopped(&mut self) {
        self.data.stopped = true;
    }

    fn on_start_notification_add(
        &mut self,
        on_start: Box<OnJobNotification>,
        job_store: JobStoreLocked,
    ) -> Result<Uuid, JobSchedulerError> {
        let job_id = self.job_id();
        let uuid = Uuid::new_v4();
        let mut js = job_store;
        js.add_notification(&job_id, &uuid, on_start, vec![JobState::Started])?;
        Ok(uuid)
    }

    fn on_start_notification_remove(
        &mut self,
        id: &Uuid,
        job_store: JobStoreLocked,
    ) -> Result<bool, JobSchedulerError> {
        let mut js = job_store;
        js.remove_notification_for_job_state(id, JobState::Started)
    }

    fn on_done_notification_add(
        &mut self,
        on_stop: Box<OnJobNotification>,
        job_store: JobStoreLocked,
    ) -> Result<Uuid, JobSchedulerError> {
        let job_id = self.job_id();
        let uuid = Uuid::new_v4();
        let mut js = job_store;
        js.add_notification(&job_id, &uuid, on_stop, vec![JobState::Done])?;
        Ok(uuid)
    }

    fn on_done_notification_remove(
        &mut self,
        id: &Uuid,
        job_store: JobStoreLocked,
    ) -> Result<bool, JobSchedulerError> {
        let mut js = job_store;
        js.remove_notification_for_job_state(id, JobState::Done)
    }

    fn on_removed_notification_add(
        &mut self,
        on_removed: Box<OnJobNotification>,
        job_store: JobStoreLocked,
    ) -> Result<Uuid, JobSchedulerError> {
        let job_id = self.job_id();
        let uuid = Uuid::new_v4();
        let mut js = job_store;
        js.add_notification(&job_id, &uuid, on_removed, vec![JobState::Removed])?;
        Ok(uuid)
    }

    fn on_removed_notification_remove(
        &mut self,
        id: &Uuid,
        job_store: JobStoreLocked,
    ) -> Result<bool, JobSchedulerError> {
        let mut js = job_store;
        js.remove_notification_for_job_state(id, JobState::Removed)
    }

    fn job_data_from_job_store(
        &mut self,
        job_store: JobStoreLocked,
    ) -> Result<Option<JobStoredData>, JobSchedulerError> {
        let mut job_store = job_store;
        let jd = job_store.get_job_data(&self.job_id())?;
        Ok(jd)
    }

    fn job_data_from_job(&mut self) -> Result<Option<JobStoredData>, JobSchedulerError> {
        Ok(Some(self.data.clone()))
    }
}
