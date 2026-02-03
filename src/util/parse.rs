use std::io::{Error, ErrorKind, Result};
use std::process::Command;

pub fn parse_size(s: &str) -> Result<u64> {
    let s = s.trim().to_lowercase();
    if s == "off" || s.is_empty() {
        return Ok(0);
    }

    if s.ends_with('%') {
        let percent_str = &s[..s.len() - 1];
        let percent: f64 = percent_str.parse().map_err(|_| {
            Error::new(ErrorKind::InvalidInput, "invalid percentage format")
        })?;
        
        if percent <= 0.0 || percent > 100.0 {
             return Err(Error::new(ErrorKind::InvalidInput, "percentage must be between 0 and 100"));
        }

        let total_mem = get_total_memory()?;
        return Ok((total_mem as f64 * (percent / 100.0)) as u64);
    }
    
    let (num_str, multiplier) = if s.ends_with("kb") || s.ends_with("k") {
        let end = if s.ends_with("kb") { 2 } else { 1 };
        (&s[..s.len() - end], 1024)
    } else if s.ends_with("mb") || s.ends_with("m") {
        let end = if s.ends_with("mb") { 2 } else { 1 };
        (&s[..s.len() - end], 1024 * 1024)
    } else if s.ends_with("gb") || s.ends_with("g") {
        let end = if s.ends_with("gb") { 2 } else { 1 };
        (&s[..s.len() - end], 1024 * 1024 * 1024)
    } else if s.ends_with("b") {
        (&s[..s.len() - 1], 1)
    } else {
         // Assume bytes if no suffix, or try parsing number directly
         match s.parse::<u64>() {
            Ok(bytes) => return Ok(bytes),
            Err(_) => return Err(Error::new(ErrorKind::InvalidInput, "invalid size format")),
         }
    };

    let num: u64 = num_str.trim().parse().map_err(|_| {
         Error::new(ErrorKind::InvalidInput, "invalid number format")
    })?;

    Ok(num * multiplier)
}

fn get_total_memory() -> Result<u64> {
    // Try cgroup v2 first (Linux container)
    if let Ok(limit) = std::fs::read_to_string("/sys/fs/cgroup/memory.max") {
         if let Ok(bytes) = limit.trim().parse::<u64>() {
             // "max" usually means unlimited, but in containers strict limits might be set.
             // If it's effectively unlimited, we might fall back to host memory or just a large number.
             // For simplicity, if it parses and is reasonable, use it. 
             // However, cgroup V2 "max" is strictly distinct from a number.
             // Let's rely on standard memory info for simplicity if not running in a strict limit container environment,
             // or implement more robust cgroup checks.
             // Re-reading /proc/meminfo is often safer for "total available to this instance" in many valid cases.
             if bytes < u64::MAX {
                 return Ok(bytes);
             }
         }
    }
    
    // Try cgroup v1
    if let Ok(limit) = std::fs::read_to_string("/sys/fs/cgroup/memory/memory.limit_in_bytes") {
        if let Ok(bytes) = limit.trim().parse::<u64>() {
             if bytes < 9223372036854771712 { // Some high value indicating no limit
                 return Ok(bytes);
             }
        }
    }

    #[cfg(target_os = "linux")]
    {
        use std::fs::File;
        use std::io::{BufRead, BufReader};
        
        let file = File::open("/proc/meminfo")?;
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = line?;
            if line.starts_with("MemTotal:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(kb) = parts[1].parse::<u64>() {
                        return Ok(kb * 1024);
                    }
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let output = Command::new("sysctl").arg("-n").arg("hw.memsize").output()?;
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout);
            if let Ok(bytes) = s.trim().parse::<u64>() {
                return Ok(bytes);
            }
        }
    }
    
    Err(Error::new(ErrorKind::Other, "could not determine total memory"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bytes() {
        assert_eq!(parse_size("100").unwrap(), 100);
        assert_eq!(parse_size("100b").unwrap(), 100);
        assert_eq!(parse_size("1k").unwrap(), 1024);
        assert_eq!(parse_size("1kb").unwrap(), 1024);
        assert_eq!(parse_size("1m").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("1mb").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("1g").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_size("1GB").unwrap(), 1024 * 1024 * 1024);
    }
    
    #[test]
    fn test_parse_off() {
        assert_eq!(parse_size("off").unwrap(), 0);
        assert_eq!(parse_size("").unwrap(), 0);
    }

    #[test]
    fn test_parse_percent() {
        let size = parse_size("50%").unwrap();
        assert!(size > 0);
        println!("50% memory is: {} bytes", size);
    }
}
