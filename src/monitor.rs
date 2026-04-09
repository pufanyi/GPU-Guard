use crate::config::MonitorConfig;
use crate::gpu::GpuStatus;

pub struct AlertInfo {
    pub message: String,
}

/// Check GPU statuses against thresholds. Returns an alert if any GPU is underutilized.
pub fn check_thresholds(gpus: &[GpuStatus], config: &MonitorConfig) -> Option<AlertInfo> {
    let mut alerts: Vec<String> = Vec::new();

    for gpu in gpus {
        let mut reasons = Vec::new();

        if gpu.gpu_utilization < config.gpu_utilization_threshold {
            reasons.push(format!(
                "GPU utilization {:.1}% < {:.1}%",
                gpu.gpu_utilization, config.gpu_utilization_threshold
            ));
        }

        if let Some(mem_threshold) = config.memory_utilization_threshold {
            if gpu.memory_utilization() < mem_threshold {
                reasons.push(format!(
                    "Memory utilization {:.1}% < {:.1}%",
                    gpu.memory_utilization(),
                    mem_threshold
                ));
            }
        }

        if !reasons.is_empty() {
            alerts.push(format!(
                "⚠️ <b>GPU {} ({})</b>\n{}",
                gpu.index,
                gpu.name,
                reasons.join("\n")
            ));
        }
    }

    if alerts.is_empty() {
        return None;
    }

    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let message = format!(
        "🖥️ <b>GPU Guard Alert</b> ({})\n\n{}\n\n{}",
        hostname,
        alerts.join("\n\n"),
        gpus.iter()
            .map(|g| g.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    );

    Some(AlertInfo { message })
}
