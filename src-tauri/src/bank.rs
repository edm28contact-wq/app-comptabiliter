use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BankTransaction {
    pub booking_date: String,
    pub value_date: Option<String>,
    pub label: String,
    pub reference: Option<String>,
    pub debit: String,
    pub credit: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BankStatement {
    pub account_label: Option<String>,
    pub period_start: Option<String>,
    pub period_end: Option<String>,
    pub opening_balance: Option<String>,
    pub closing_balance: Option<String>,
    pub transactions: Vec<BankTransaction>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StatementCheck {
    pub balanced: Option<bool>,
    pub opening_balance: Option<String>,
    pub movement_total: String,
    pub expected_closing_balance: Option<String>,
    pub closing_balance: Option<String>,
    pub errors: Vec<String>,
}

fn parse_amount(value: &str) -> Result<f64, String> {
    let cleaned = value
        .replace('€', "")
        .replace("EUR", "")
        .replace('\u{00a0}', "")
        .replace(' ', "");

    let normalized = if cleaned.contains(',') {
        cleaned.replace('.', "").replace(',', ".")
    } else {
        cleaned
    };

    normalized
        .parse::<f64>()
        .map_err(|_| format!("Montant bancaire invalide : {value}"))
}

fn format_amount(value: f64) -> String {
    format!("{value:.2}")
}

pub fn check_statement(statement: &BankStatement) -> StatementCheck {
    let mut errors = Vec::new();
    let mut movements = 0.0_f64;

    for (index, transaction) in statement.transactions.iter().enumerate() {
        let debit = match parse_amount(&transaction.debit) {
            Ok(value) if value >= 0.0 => value,
            Ok(_) => {
                errors.push(format!("Mouvement {} : débit négatif.", index + 1));
                0.0
            }
            Err(error) => {
                errors.push(format!("Mouvement {} : {error}", index + 1));
                0.0
            }
        };
        let credit = match parse_amount(&transaction.credit) {
            Ok(value) if value >= 0.0 => value,
            Ok(_) => {
                errors.push(format!("Mouvement {} : crédit négatif.", index + 1));
                0.0
            }
            Err(error) => {
                errors.push(format!("Mouvement {} : {error}", index + 1));
                0.0
            }
        };
        if debit > 0.005 && credit > 0.005 {
            errors.push(format!("Mouvement {} : débit et crédit renseignés simultanément.", index + 1));
        }
        movements += credit - debit;
    }

    let opening = statement.opening_balance.as_deref().and_then(|value| match parse_amount(value) {
        Ok(amount) => Some(amount),
        Err(error) => {
            errors.push(error);
            None
        }
    });
    let closing = statement.closing_balance.as_deref().and_then(|value| match parse_amount(value) {
        Ok(amount) => Some(amount),
        Err(error) => {
            errors.push(error);
            None
        }
    });

    let expected = opening.map(|value| value + movements);
    let balanced = match (expected, closing) {
        (Some(expected), Some(closing)) => Some((expected - closing).abs() <= 0.02),
        _ => None,
    };

    if balanced == Some(false) {
        errors.push("Solde d'ouverture + mouvements ≠ solde de clôture.".to_string());
    }

    StatementCheck {
        balanced,
        opening_balance: opening.map(format_amount),
        movement_total: format_amount(movements),
        expected_closing_balance: expected.map(format_amount),
        closing_balance: closing.map(format_amount),
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_statement() -> BankStatement {
        BankStatement {
            account_label: Some("Compte courant".to_string()),
            period_start: Some("01/08/2026".to_string()),
            period_end: Some("31/08/2026".to_string()),
            opening_balance: Some("1000,00".to_string()),
            closing_balance: Some("236,28".to_string()),
            transactions: vec![
                BankTransaction {
                    booking_date: "15/08/2026".to_string(),
                    value_date: None,
                    label: "PRLV EDF".to_string(),
                    reference: Some("874521".to_string()),
                    debit: "1248,72".to_string(),
                    credit: "0".to_string(),
                },
                BankTransaction {
                    booking_date: "20/08/2026".to_string(),
                    value_date: None,
                    label: "VIREMENT".to_string(),
                    reference: None,
                    debit: "0".to_string(),
                    credit: "500,00".to_string(),
                },
                BankTransaction {
                    booking_date: "31/08/2026".to_string(),
                    value_date: None,
                    label: "FRAIS TENUE DE COMPTE".to_string(),
                    reference: None,
                    debit: "15,00".to_string(),
                    credit: "0".to_string(),
                },
            ],
        }
    }

    #[test]
    fn validates_balanced_statement() {
        let check = check_statement(&sample_statement());
        assert_eq!(check.movement_total, "-763.72");
        assert_eq!(check.expected_closing_balance.as_deref(), Some("236.28"));
        assert_eq!(check.balanced, Some(true));
        assert!(check.errors.is_empty());
    }

    #[test]
    fn detects_wrong_closing_balance() {
        let mut statement = sample_statement();
        statement.closing_balance = Some("250,00".to_string());
        let check = check_statement(&statement);
        assert_eq!(check.balanced, Some(false));
        assert!(check.errors.iter().any(|error| error.contains("solde de clôture")));
    }

    #[test]
    fn accepts_french_thousands_separator() {
        assert_eq!(parse_amount("1.248,72").unwrap(), 1248.72);
    }
}
