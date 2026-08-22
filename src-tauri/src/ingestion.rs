use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path, time::UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSnapshot {
    pub size: u64,
    pub modified_ms: u128,
}

pub fn snapshot(path: &Path) -> Result<FileSnapshot, String> {
    let metadata = path.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("Le chemin n'est pas un fichier.".to_string());
    }
    let modified = metadata
        .modified()
        .map_err(|error| error.to_string())?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    Ok(FileSnapshot { size: metadata.len(), modified_ms: modified })
}

pub fn stable_observation_count(previous: Option<&FileSnapshot>, current: &FileSnapshot, previous_count: u32) -> u32 {
    match previous {
        Some(previous) if previous == current => previous_count.saturating_add(1),
        _ => 1,
    }
}

pub fn is_stable(observation_count: u32) -> bool {
    observation_count >= 2
}

pub fn sha256(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_two_identical_observations() {
        let snapshot = FileSnapshot { size: 1024, modified_ms: 42 };
        let first = stable_observation_count(None, &snapshot, 0);
        let second = stable_observation_count(Some(&snapshot), &snapshot, first);
        assert_eq!(first, 1);
        assert_eq!(second, 2);
        assert!(!is_stable(first));
        assert!(is_stable(second));
    }

    #[test]
    fn resets_when_size_changes() {
        let previous = FileSnapshot { size: 1024, modified_ms: 42 };
        let current = FileSnapshot { size: 2048, modified_ms: 43 };
        assert_eq!(stable_observation_count(Some(&previous), &current, 5), 1);
    }
}
