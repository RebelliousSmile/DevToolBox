//! Strict protocol-boundary validation over tolerant serde mirrors.

use super::model::{CatalogSnapshot, ProgressEvent};

const SCHEMA_MAJOR: u64 = 1;

fn validate_schema(version: u64) -> Result<(), String> {
    if version != SCHEMA_MAJOR {
        return Err(format!(
            "version majeure du protocole modèles non prise en charge: {version}"
        ));
    }
    Ok(())
}

pub fn parse_snapshot(json: &str) -> Result<CatalogSnapshot, String> {
    let snapshot: CatalogSnapshot = serde_json::from_str(json)
        .map_err(|error| format!("catalogue modèles JSON invalide: {error}"))?;
    validate_schema(snapshot.schema_version)?;
    Ok(snapshot)
}

#[derive(Debug)]
pub struct ProgressValidator {
    operation_id: String,
    last_sequence: u64,
    last_bytes: u64,
    terminal: bool,
}

impl ProgressValidator {
    pub fn new(operation_id: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            last_sequence: 0,
            last_bytes: 0,
            terminal: false,
        }
    }

    pub fn parse_line(&mut self, line: &str) -> Result<ProgressEvent, String> {
        let event: ProgressEvent = serde_json::from_str(line)
            .map_err(|error| format!("événement modèles NDJSON invalide: {error}"))?;
        validate_schema(event.schema_version)?;
        if event.operation_id != self.operation_id {
            return Err(format!(
                "operation_id inattendu: attendu {}, reçu {}",
                self.operation_id, event.operation_id
            ));
        }
        if self.terminal {
            return Err("événement reçu après l'événement terminal".to_string());
        }
        if event.sequence <= self.last_sequence {
            return Err("séquence d'événements non monotone".to_string());
        }
        if let Some(transferred) = event.transferred_bytes {
            if transferred < self.last_bytes {
                return Err("compteur d'octets en recul".to_string());
            }
            if event.total_bytes.is_some_and(|total| transferred > total) {
                return Err("compteur d'octets supérieur au total".to_string());
            }
            self.last_bytes = transferred;
        }
        if !matches!(
            event.kind.as_str(),
            "schema" | "progress" | "completed" | "failed" | "cancelled"
        ) {
            return Err(format!("type d'événement inconnu: {}", event.kind));
        }
        self.last_sequence = event.sequence;
        self.terminal = event.is_terminal();
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(sequence: u64, operation: &str, bytes: u64, kind: &str) -> String {
        format!(
            r#"{{"schema_version":1,"sequence":{sequence},"kind":"{kind}","operation_id":"{operation}","transferred_bytes":{bytes}}}"#
        )
    }

    #[test]
    fn rejects_unknown_schema_and_malformed_json() {
        assert!(parse_snapshot(r#"{"schema_version":2}"#).is_err());
        assert!(parse_snapshot("{").is_err());
    }

    #[test]
    fn rejects_mismatch_regression_and_duplicate_terminal() {
        let mut parser = ProgressValidator::new("op");
        assert!(parser.parse_line(&event(1, "other", 0, "schema")).is_err());
        parser.parse_line(&event(1, "op", 10, "progress")).unwrap();
        assert!(parser.parse_line(&event(2, "op", 9, "progress")).is_err());

        let mut parser = ProgressValidator::new("op");
        parser.parse_line(&event(1, "op", 10, "completed")).unwrap();
        assert!(parser.parse_line(&event(2, "op", 10, "failed")).is_err());
    }
}
