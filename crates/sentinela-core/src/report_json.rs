use crate::model::Report;
use crate::Result;

/// Serialização determinística (pretty) do relatório canônico.
pub fn serialize(report: &Report) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(report)?)
}
