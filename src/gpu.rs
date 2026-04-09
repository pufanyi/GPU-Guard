use std::process::Command;

#[derive(Debug, Clone)]
pub struct GpuStatus {
    pub index: u32,
    pub name: String,
    /// GPU utilization percentage (0-100)
    pub gpu_utilization: f64,
    /// Memory used in MiB
    pub memory_used: f64,
    /// Total memory in MiB
    pub memory_total: f64,
}

impl GpuStatus {
    pub fn memory_utilization(&self) -> f64 {
        if self.memory_total == 0.0 {
            return 0.0;
        }
        self.memory_used / self.memory_total * 100.0
    }
}

impl std::fmt::Display for GpuStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GPU {} ({}): utilization={:.1}%, memory={:.0}/{:.0} MiB ({:.1}%)",
            self.index,
            self.name,
            self.gpu_utilization,
            self.memory_used,
            self.memory_total,
            self.memory_utilization(),
        )
    }
}

/// Query all GPUs via nvidia-smi.
pub fn query_gpus() -> anyhow::Result<Vec<GpuStatus>> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,utilization.gpu,memory.used,memory.total,name",
            "--format=csv,noheader,nounits",
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("nvidia-smi failed: {stderr}");
    }

    let stdout = String::from_utf8(output.stdout)?;
    let mut gpus = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if parts.len() < 5 {
            continue;
        }
        gpus.push(GpuStatus {
            index: parts[0].parse()?,
            gpu_utilization: parts[1].parse()?,
            memory_used: parts[2].parse()?,
            memory_total: parts[3].parse()?,
            name: parts[4].to_string(),
        });
    }

    Ok(gpus)
}
