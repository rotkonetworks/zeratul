//! CPU core affinity utilities for benchmarking
//!
//! Detects physical/performance cores and sets thread affinity.
//! - Linux: detects physical cores (excludes SMT siblings)
//! - macOS: detects P-cores (performance cores on Apple Silicon)

use core_affinity::CoreId;

#[cfg(target_os = "linux")]
pub fn get_physical_cores() -> Vec<usize> {
    use std::collections::HashMap;
    use std::fs;

    let mut sibling_map: HashMap<Vec<usize>, Vec<usize>> = HashMap::new();

    if let Ok(entries) = fs::read_dir("/sys/devices/system/cpu") {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("cpu") && name[3..].parse::<usize>().is_ok() {
                    let sibling_path = path.join("topology/thread_siblings_list");
                    if let Ok(content) = fs::read_to_string(&sibling_path) {
                        let mut siblings: Vec<usize> = content
                            .trim()
                            .split(',')
                            .filter_map(|s| s.parse().ok())
                            .collect();
                        siblings.sort_unstable();

                        if !siblings.is_empty() {
                            let cpu: usize = name[3..].parse().unwrap();
                            sibling_map.entry(siblings).or_default().push(cpu);
                        }
                    }
                }
            }
        }
    }

    let mut physical: Vec<usize> = sibling_map
        .values()
        .filter_map(|group| group.iter().min().copied())
        .collect();

    physical.sort_unstable();
    physical.dedup();
    physical
}

#[cfg(target_os = "macos")]
pub fn get_physical_cores() -> Vec<usize> {
    use std::process::Command;

    // get performance core count (P-cores on Apple Silicon)
    let output = Command::new("sysctl")
        .arg("-n")
        .arg("hw.perflevel0.physicalcpu")
        .output();

    if let Ok(output) = output {
        if let Ok(count_str) = std::str::from_utf8(&output.stdout) {
            if let Ok(count) = count_str.trim().parse::<usize>() {
                // p-cores are typically cores 0..count
                return (0..count).collect();
            }
        }
    }

    // fallback: return all cores
    core_affinity::get_core_ids()
        .unwrap_or_default()
        .into_iter()
        .map(|id| id.id)
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn get_physical_cores() -> Vec<usize> {
    core_affinity::get_core_ids()
        .unwrap_or_default()
        .into_iter()
        .map(|id| id.id)
        .collect()
}

/// Pin current thread to first physical/performance core
pub fn pin_to_physical_core() -> bool {
    let physical_cores = get_physical_cores();
    if !physical_cores.is_empty() {
        core_affinity::set_for_current(CoreId { id: physical_cores[0] })
    } else {
        false
    }
}

/// Pin current thread to specific physical core index
pub fn pin_to_core(core_idx: usize) -> bool {
    let physical_cores = get_physical_cores();
    if core_idx < physical_cores.len() {
        core_affinity::set_for_current(CoreId { id: physical_cores[core_idx] })
    } else {
        false
    }
}

/// Get number of physical/performance cores
pub fn num_physical_cores() -> usize {
    get_physical_cores().len()
}
