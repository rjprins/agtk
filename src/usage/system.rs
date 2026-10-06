//! Whole-machine usage from Linux procfs, independent of agent processes.

#[derive(Debug, Clone, Copy)]
pub struct CpuSample {
    total: u64,
    idle: u64,
}

impl CpuSample {
    pub fn parse(stat: &str) -> Option<Self> {
        let line = stat.lines().find(|line| line.starts_with("cpu "))?;
        let values = line
            .split_whitespace()
            .skip(1)
            .take(8)
            .map(str::parse::<u64>)
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        if values.len() < 4 {
            return None;
        }
        // guest and guest_nice are already included in user and nice.
        let total = values
            .iter()
            .try_fold(0_u64, |sum, value| sum.checked_add(*value))?;
        let idle = values[3].checked_add(values.get(4).copied().unwrap_or(0))?;
        Some(Self { total, idle })
    }

    pub fn percent_since(self, previous: Self) -> Option<f64> {
        let total = self.total.checked_sub(previous.total)?;
        let idle = self.idle.checked_sub(previous.idle)?;
        (total > 0 && idle <= total).then(|| 100.0 * (total - idle) as f64 / total as f64)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Memory {
    pub used: u64,
    pub total: u64,
}

impl Memory {
    pub fn parse(meminfo: &str) -> Option<Self> {
        let value = |name: &str| {
            meminfo.lines().find_map(|line| {
                let mut fields = line.split_whitespace();
                if fields.next()? != name {
                    return None;
                }
                let kib = fields.next()?.parse::<u64>().ok()?;
                (fields.next()? == "kB").then_some(kib)?.checked_mul(1024)
            })
        };
        let total = value("MemTotal:")?;
        let used = total.checked_sub(value("MemAvailable:")?)?;
        (total > 0).then_some(Self { used, total })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_uses_sample_deltas_and_does_not_count_guests_twice() {
        let first = CpuSample::parse("cpu  100 0 50 800 50 0 0 0 20 0").unwrap();
        let second = CpuSample::parse("cpu  130 0 60 850 60 0 0 0 30 0").unwrap();
        assert_eq!(second.percent_since(first), Some(40.0));
        assert_eq!(first.percent_since(first), None);
        assert_eq!(first.percent_since(second), None);
    }

    #[test]
    fn memory_uses_available_including_reclaimable_cache() {
        let memory =
            Memory::parse("MemTotal: 1000 kB\nMemFree: 100 kB\nMemAvailable: 400 kB\n").unwrap();
        assert_eq!(memory.used, 600 * 1024);
        assert_eq!(memory.total, 1000 * 1024);
        assert!(Memory::parse("MemTotal: 1000 kB\n").is_none());
        assert!(Memory::parse("MemTotal: 100 kB\nMemAvailable: 200 kB\n").is_none());
    }
}
