use crate::error::JobSchedulerError;
use crate::job::{JobLocked, JobType};
use chrono::Utc;
use std::error::Error;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::task::JoinHandle;
use uuid::Uuid;

pub trait JobScheduler {
    /// Add a job to the `JobScheduler`
    fn add(&mut self, job: JobLocked) -> Result<(), Box<dyn std::error::Error + '_>>;

    /// Remove a job from the `JobScheduler`
    fn remove(&mut self, to_be_removed: &Uuid) -> Result<(), Box<dyn std::error::Error + '_>>;

    /// The `tick` method increments time for the JobScheduler and executes
    /// any pending jobs.
    fn tick(
        &mut self,
        scheduler: JobsSchedulerLocked,
    ) -> Result<(), Box<dyn std::error::Error + '_>>;

    /// The `time_till_next_job` method returns the duration till the next job
    /// is supposed to run. This can be used to sleep until then without waking
    /// up at a fixed interval.
    fn time_till_next_job(&self) -> Result<std::time::Duration, Box<dyn std::error::Error + '_>>;
}

pub trait JobSchedulerLockedType: JobScheduler {}

/// The JobScheduler contains and executes the scheduled jobs.
pub struct JobsSchedulerLocked(Arc<RwLock<Box<dyn JobSchedulerLockedType + Send + Sync>>>);

impl Clone for JobsSchedulerLocked {
    fn clone(&self) -> Self {
        JobsSchedulerLocked(self.0.clone())
    }
}

#[derive(Default, Clone)]
pub struct SimpleJobScheduler {
    jobs: Vec<JobLocked>,
}

unsafe impl Send for SimpleJobScheduler {}
unsafe impl Sync for SimpleJobScheduler {}

impl JobScheduler for SimpleJobScheduler {
    fn add(&mut self, job: JobLocked) -> Result<(), Box<dyn Error + '_>> {
        self.jobs.push(job);
        Ok(())
    }

    fn remove(&mut self, to_be_removed: &Uuid) -> Result<(), Box<dyn Error + '_>> {
        {
            let mut removed: Vec<JobLocked> = vec![];
            self.jobs.retain(|f| !{
                let not_to_be_removed = if let Ok(f) = f.0.read() {
                    f.job_id().eq(to_be_removed)
                } else {
                    false
                };
                if !not_to_be_removed {
                    let f = f.0.clone();
                    removed.push(JobLocked(f))
                }
                not_to_be_removed
            });
            for job in removed {
                let mut job_r = job.0.write().unwrap();
                job_r.set_stopped();
                let job_type = job_r.job_type();
                if matches!(job_type, JobType::OneShot) || matches!(job_type, JobType::Repeated) {
                    job_r.abort_join_handle();
                }
            }
        }
        Ok(())
    }

    fn tick(&mut self, scheduler: JobsSchedulerLocked) -> Result<(), Box<dyn Error + '_>> {
        for jl in self.jobs.iter_mut() {
            if jl.tick() {
                let ref_for_later = jl.0.clone();
                let jobs = scheduler.clone();
                tokio::spawn(async move {
                    let e = ref_for_later.write();
                    if let Ok(mut w) = e {
                        let jt = w.job_type();
                        if matches!(jt, JobType::OneShot) {
                            let mut jobs = jobs.clone();
                            let job_id = w.job_id();
                            tokio::spawn(async move {
                                if let Err(e) = jobs.remove(&job_id) {
                                    eprintln!("Error removing job {:?}", e);
                                }
                            });
                        }
                        w.run(jobs);
                    }
                });
            }
        }

        Ok(())
    }

    fn time_till_next_job(&self) -> Result<Duration, Box<dyn Error + '_>> {
        if self.jobs.is_empty() {
            // Take a guess if there are no jobs.
            return Ok(std::time::Duration::from_millis(500));
        }
        let now = Utc::now();
        let min = self
            .jobs
            .iter()
            .map(|j| {
                let diff = {
                    j.0.read().ok().and_then(|j| {
                        j.schedule().and_then(|s| {
                            s.upcoming(Utc)
                                .take(1)
                                .find(|_| true)
                                .map(|next| next - now)
                        })
                    })
                };
                diff
            })
            .flatten()
            .min();

        let m = min
            .unwrap_or_else(chrono::Duration::zero)
            .to_std()
            .unwrap_or_else(|_| std::time::Duration::new(0, 0));
        Ok(m)
    }
}
impl JobSchedulerLockedType for SimpleJobScheduler {}

impl Default for JobsSchedulerLocked {
    fn default() -> Self {
        Self::new()
    }
}

impl JobsSchedulerLocked {
    /// Create a new `JobSchedulerLocked`.
    pub fn new() -> JobsSchedulerLocked {
        let r = SimpleJobScheduler {
            ..Default::default()
        };
        JobsSchedulerLocked(Arc::new(RwLock::new(Box::new(r))))
    }

    /// Add a job to the `JobScheduler`
    ///
    /// ```rust,ignore
    /// use tokio_cron_scheduler::{Job, JobScheduler, JobToRun};
    /// let mut sched = JobScheduler::new();
    /// sched.add(Job::new("1/10 * * * * *".parse().unwrap(), || {
    ///     println!("I get executed every 10 seconds!");
    /// }));
    /// ```
    pub fn add(&mut self, job: JobLocked) -> Result<(), JobSchedulerError> {
        {
            let mut self_w = self.0.write().map_err(|_e| JobSchedulerError::CantAdd)?;
            if self_w.add(job).is_err() {
                return Err(JobSchedulerError::CantAdd);
            }
        }
        Ok(())
    }

    /// Remove a job from the `JobScheduler`
    ///
    /// ```rust,ignore
    /// use tokio_cron_scheduler::{Job, JobScheduler, JobToRun};
    /// let mut sched = JobScheduler::new();
    /// let job_id = sched.add(Job::new("1/10 * * * * *".parse().unwrap(), || {
    ///     println!("I get executed every 10 seconds!");
    /// }));
    /// sched.remove(job_id);
    /// ```
    pub fn remove(&mut self, to_be_removed: &Uuid) -> Result<(), JobSchedulerError> {
        let mut ws = self.0.write().map_err(|_| JobSchedulerError::CantRemove)?;
        if ws.remove(to_be_removed).is_err() {
            return Err(JobSchedulerError::CantRemove);
        }
        Ok(())
    }

    /// The `tick` method increments time for the JobScheduler and executes
    /// any pending jobs. It is recommended to sleep for at least 500
    /// milliseconds between invocations of this method.
    /// This is kept public if you're running this yourself. It is better to
    /// call the `start` method if you want all of this automated for you.
    ///
    /// ```rust,ignore
    /// loop {
    ///     sched.tick();
    ///     std::thread::sleep(Duration::from_millis(500));
    /// }
    /// ```
    pub fn tick(&mut self) -> Result<(), JobSchedulerError> {
        let mut ws = self.0.write().map_err(|_| JobSchedulerError::TickError)?;
        if ws.tick(self.clone()).is_err() {
            return Err(JobSchedulerError::TickError);
        }
        Ok(())
    }

    /// The `start` spawns a Tokio task where it loops. Every 500ms it
    /// runs the tick method to increment any
    /// any pending jobs.
    ///
    /// ```rust,ignore
    /// if let Err(e) = sched.start().await {
    ///         eprintln!("Error on scheduler {:?}", e);
    ///     }
    /// ```
    pub fn start(&self) -> JoinHandle<()> {
        let jl: JobsSchedulerLocked = self.clone();
        let jh: JoinHandle<()> = tokio::spawn(async move {
            loop {
                tokio::time::sleep(core::time::Duration::from_millis(500)).await;
                let mut jsl = jl.clone();
                let tick = jsl.tick();
                if let Err(e) = tick {
                    eprintln!("Error on job scheduler tick {:?}", e);
                    break;
                }
            }
        });
        jh
    }

    /// The `time_till_next_job` method returns the duration till the next job
    /// is supposed to run. This can be used to sleep until then without waking
    /// up at a fixed interval.AsMut
    ///
    /// ```rust, ignore
    /// loop {
    ///     sched.tick();
    ///     std::thread::sleep(sched.time_till_next_job());
    /// }
    /// ```
    pub fn time_till_next_job(&self) -> Result<std::time::Duration, JobSchedulerError> {
        let r = self
            .0
            .read()
            .map_err(|_| JobSchedulerError::CantGetTimeUntil)?;
        let l = r
            .time_till_next_job()
            .map_err(|_| JobSchedulerError::CantGetTimeUntil)?;
        Ok(l)
    }
}
