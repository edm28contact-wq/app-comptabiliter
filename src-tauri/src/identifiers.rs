pub fn normalize_siret(value: &str) -> Option<String> {
    let digits: String = value.chars().filter(|character| character.is_ascii_digit()).collect();
    if digits.len() == 14 { Some(digits) } else { None }
}

pub fn is_valid_siret(value: &str) -> bool {
    let Some(digits) = normalize_siret(value) else { return false; };
    let mut sum = 0_u32;
    for (index, byte) in digits.bytes().enumerate() {
        let mut digit = (byte - b'0') as u32;
        if index % 2 == 0 {
            digit *= 2;
            if digit > 9 { digit -= 9; }
        }
        sum += digit;
    }
    sum % 10 == 0
}

pub fn normalize_iban(value: &str) -> Option<String> {
    let normalized: String = value
        .chars()
        .filter(|character| !character.is_whitespace())
        .map(|character| character.to_ascii_uppercase())
        .collect();
    if (15..=34).contains(&normalized.len())
        && normalized.chars().all(|character| character.is_ascii_alphanumeric())
        && normalized.chars().take(2).all(|character| character.is_ascii_alphabetic())
        && normalized.chars().skip(2).take(2).all(|character| character.is_ascii_digit())
    {
        Some(normalized)
    } else {
        None
    }
}

pub fn is_valid_iban(value: &str) -> bool {
    let Some(iban) = normalize_iban(value) else { return false; };
    let rearranged = format!("{}{}", &iban[4..], &iban[..4]);
    let mut remainder = 0_u32;
    for character in rearranged.chars() {
        if character.is_ascii_digit() {
            remainder = (remainder * 10 + character.to_digit(10).unwrap()) % 97;
        } else if character.is_ascii_uppercase() {
            let value = character as u32 - 'A' as u32 + 10;
            remainder = (remainder * 100 + value) % 97;
        } else {
            return false;
        }
    }
    remainder == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_known_siret() {
        assert!(is_valid_siret("732 829 320 00074"));
    }

    #[test]
    fn rejects_bad_siret_checksum() {
        assert!(!is_valid_siret("73282932000075"));
    }

    #[test]
    fn validates_known_iban() {
        assert!(is_valid_iban("FR14 2004 1010 0505 0001 3M02 606"));
    }

    #[test]
    fn rejects_bad_iban_checksum() {
        assert!(!is_valid_iban("FR15 2004 1010 0505 0001 3M02 606"));
    }
}
