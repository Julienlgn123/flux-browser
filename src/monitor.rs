//! Mesure de la consommation (mémoire, processeur) des processus du navigateur
//! et plafond processeur matériel via un Job Object Windows.

use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

use windows::Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectCpuRateControlInformation, SetInformationJobObject,
            JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
            JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
        },
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX},
        SystemInformation::{GetSystemInfo, SYSTEM_INFO},
        Threading::{
            GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};

#[derive(Clone, Copy, Default)]
pub struct Usage {
    /// Mémoire privée, en octets.
    pub mem: u64,
    /// Processeur, en % de la machine entière (comme le Gestionnaire des tâches).
    pub cpu: f64,
}

pub struct Sampler {
    cores: f64,
    prev: HashMap<u32, (u64, Instant)>,
}

fn ft(t: FILETIME) -> u64 {
    ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
}

impl Sampler {
    pub fn new() -> Self {
        let mut info = SYSTEM_INFO::default();
        unsafe { GetSystemInfo(&mut info) };
        Self { cores: info.dwNumberOfProcessors.max(1) as f64, prev: HashMap::new() }
    }

    pub fn cores(&self) -> f64 {
        self.cores
    }

    pub fn sample(&mut self, pid: u32) -> Usage {
        unsafe {
            let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return Usage::default();
            };
            let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
            let mem = GetProcessMemoryInfo(
                h,
                &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            )
            .map(|_| c.PrivateUsage as u64)
            .unwrap_or(0);
            let (mut a, mut b, mut k, mut u) = Default::default();
            let busy = GetProcessTimes(h, &mut a, &mut b, &mut k, &mut u).map(|_| ft(k) + ft(u)).unwrap_or(0);
            let _ = CloseHandle(h);
            let now = Instant::now();
            let cpu = match self.prev.insert(pid, (busy, now)) {
                Some((before, at)) => {
                    let wall = now.duration_since(at).as_secs_f64() * 1e7; // unités de 100 ns
                    if wall > 0.0 {
                        (busy.saturating_sub(before) as f64 / wall / self.cores * 100.0).min(100.0)
                    } else {
                        0.0
                    }
                }
                None => 0.0,
            };
            Usage { mem, cpu }
        }
    }

    /// Oublie les processus disparus.
    pub fn retain(&mut self, alive: &HashSet<u32>) {
        self.prev.retain(|pid, _| alive.contains(pid));
    }
}

/// Plafond processeur appliqué par Windows à tous les processus du navigateur.
///
/// Les processus de rendu de Chromium sont isolés (bac à sable) et refusent d'être
/// rattachés après coup : Flux entre donc lui-même dans le job avant de démarrer le
/// moteur, et tous les processus qu'il lance en héritent.
pub struct CpuLimiter {
    job: HANDLE,
    percent: u32,
}

impl CpuLimiter {
    /// À appeler avant la création de la première vue WebView2.
    pub fn new() -> Option<Self> {
        unsafe {
            let job = CreateJobObjectW(None, None).ok()?;
            AssignProcessToJobObject(job, GetCurrentProcess()).ok()?;
            Some(Self { job, percent: u32::MAX })
        }
    }

    pub fn set(&mut self, percent: u32) {
        if percent == self.percent {
            return;
        }
        self.percent = percent;
        let mut info = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION::default();
        if percent > 0 {
            info.ControlFlags = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
            // Unité Windows : centièmes de pourcent du temps processeur total.
            info.Anonymous.CpuRate = percent.clamp(1, 100) * 100;
        }
        unsafe {
            let _ = SetInformationJobObject(
                self.job,
                JobObjectCpuRateControlInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
            );
        }
    }
}
