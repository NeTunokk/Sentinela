use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::ObservedValue;

/// Operações suportadas em `expect.op`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectOp {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    IsTrue,
    IsFalse,
    NotNull,
}

impl ExpectOp {
    #[must_use]
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            Self::Eq | Self::Ne | Self::Gt | Self::Gte | Self::Lt | Self::Lte
        )
    }
}

/// Expectativa declarativa de um teste SQL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expectation {
    pub op: ExpectOp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

impl Expectation {
    /// Valida a combinação operação/valor. Não executa SQL.
    pub fn validate(&self) -> Result<(), String> {
        match (self.op, self.value.as_ref()) {
            (ExpectOp::NotNull, Some(_)) => Err("op 'not_null' não aceita value".into()),
            (ExpectOp::NotNull, None) => Ok(()),
            (ExpectOp::IsTrue | ExpectOp::IsFalse, Some(_)) => {
                Err("op 'is_true'/'is_false' não aceita value".into())
            }
            (ExpectOp::IsTrue | ExpectOp::IsFalse, None) => Ok(()),
            (op, None) if op.is_comparison() => Err(format!("op '{}' exige value", op_name(op))),
            (op, Some(Value::Bool(_))) if op.is_comparison() => Ok(()),
            (op, Some(Value::Number(_))) if op.is_comparison() => Ok(()),
            (op, Some(_)) if op.is_comparison() => {
                Err("value deve ser numérico ou booleano".into())
            }
            _ => Ok(()),
        }
    }

    /// Avalia a expectativa contra o primeiro valor escalar retornado.
    ///
    /// Mensagens de erro nunca incluem o valor observado nem linhas do banco.
    pub fn evaluate(&self, observed: Option<&ObservedValue>) -> Result<(), String> {
        self.validate()?;

        if self.op == ExpectOp::NotNull {
            return observed.map(|_| ()).ok_or_else(|| "valor nulo".to_string());
        }

        let observed = observed.ok_or_else(|| "valor nulo".to_string())?;

        match self.op {
            ExpectOp::Eq
            | ExpectOp::Ne
            | ExpectOp::Gt
            | ExpectOp::Gte
            | ExpectOp::Lt
            | ExpectOp::Lte => {
                let expected = self
                    .value
                    .as_ref()
                    .ok_or_else(|| "expectativa sem value".to_string())?;
                let ordering = compare(observed, expected)?;
                let passed = match self.op {
                    ExpectOp::Eq => ordering == Ordering::Equal,
                    ExpectOp::Ne => ordering != Ordering::Equal,
                    ExpectOp::Gt => ordering == Ordering::Greater,
                    ExpectOp::Gte => ordering != Ordering::Less,
                    ExpectOp::Lt => ordering == Ordering::Less,
                    ExpectOp::Lte => ordering != Ordering::Greater,
                    _ => unreachable!("braço restrito a comparações"),
                };
                if passed {
                    Ok(())
                } else {
                    Err(format!("expectativa '{}' não satisfeita", op_name(self.op)))
                }
            }
            ExpectOp::IsTrue => match observed {
                ObservedValue::Bool(true) => Ok(()),
                ObservedValue::Bool(false) => Err("valor booleano falso".into()),
                _ => Err("valor não é booleano".into()),
            },
            ExpectOp::IsFalse => match observed {
                ObservedValue::Bool(false) => Ok(()),
                ObservedValue::Bool(true) => Err("valor booleano verdadeiro".into()),
                _ => Err("valor não é booleano".into()),
            },
            ExpectOp::NotNull => Ok(()),
        }
    }
}

fn op_name(op: ExpectOp) -> &'static str {
    match op {
        ExpectOp::Eq => "eq",
        ExpectOp::Ne => "ne",
        ExpectOp::Gt => "gt",
        ExpectOp::Gte => "gte",
        ExpectOp::Lt => "lt",
        ExpectOp::Lte => "lte",
        ExpectOp::IsTrue => "is_true",
        ExpectOp::IsFalse => "is_false",
        ExpectOp::NotNull => "not_null",
    }
}

fn compare(observed: &ObservedValue, expected: &Value) -> Result<Ordering, String> {
    match (observed, expected) {
        (ObservedValue::Bool(left), Value::Bool(right)) => Ok(left.cmp(right)),
        (ObservedValue::Bool(_), _) => Err("valor booleano não pode ser comparado a número".into()),
        (ObservedValue::Int(left), Value::Number(right)) => compare_int(*left, right),
        (ObservedValue::Float(left), Value::Number(right)) => {
            let right = right
                .as_f64()
                .ok_or_else(|| "número esperado inválido".to_string())?;
            left.partial_cmp(&right)
                .ok_or_else(|| "comparação numérica inválida".to_string())
        }
        (ObservedValue::Int(_) | ObservedValue::Float(_), Value::Bool(_)) => {
            Err("valor numérico não pode ser comparado a booleano".into())
        }
        (_, _) => Err("tipo de value não suportado".into()),
    }
}

fn compare_int(left: i64, right: &serde_json::Number) -> Result<Ordering, String> {
    if let Some(right) = right.as_i64() {
        return Ok(left.cmp(&right));
    }
    if let Some(right) = right.as_u64() {
        if left < 0 {
            return Ok(Ordering::Less);
        }
        return Ok((left as u64).cmp(&right));
    }
    if let Some(right) = right.as_f64() {
        return (left as f64)
            .partial_cmp(&right)
            .ok_or_else(|| "comparação numérica inválida".to_string());
    }
    Err("número esperado inválido".into())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn expect(op: ExpectOp, value: Option<Value>) -> Expectation {
        Expectation { op, value }
    }

    #[test]
    fn validates_value_requirements() {
        assert!(expect(ExpectOp::Gt, None).validate().is_err());
        assert!(expect(ExpectOp::Gt, Some(json!("x"))).validate().is_err());
        assert!(expect(ExpectOp::NotNull, Some(json!(1)))
            .validate()
            .is_err());
        assert!(expect(ExpectOp::IsTrue, None).validate().is_ok());
        assert!(expect(ExpectOp::Eq, Some(json!(0))).validate().is_ok());
    }

    #[test]
    fn evaluates_numeric_operations() {
        let value = ObservedValue::Int(10);
        assert!(expect(ExpectOp::Eq, Some(json!(10)))
            .evaluate(Some(&value))
            .is_ok());
        assert!(expect(ExpectOp::Gt, Some(json!(9)))
            .evaluate(Some(&value))
            .is_ok());
        assert!(expect(ExpectOp::Lt, Some(json!(9)))
            .evaluate(Some(&value))
            .is_err());
        assert!(expect(ExpectOp::Ne, Some(json!(11)))
            .evaluate(Some(&value))
            .is_ok());
        assert!(expect(ExpectOp::Gte, Some(json!(10)))
            .evaluate(Some(&value))
            .is_ok());
        assert!(expect(ExpectOp::Lte, Some(json!(10.0)))
            .evaluate(Some(&value))
            .is_ok());
    }

    #[test]
    fn evaluates_booleans_and_null() {
        assert!(expect(ExpectOp::IsTrue, None)
            .evaluate(Some(&ObservedValue::Bool(true)))
            .is_ok());
        assert!(expect(ExpectOp::IsFalse, None)
            .evaluate(Some(&ObservedValue::Bool(false)))
            .is_ok());
        assert!(expect(ExpectOp::NotNull, None).evaluate(None).is_err());
        assert!(expect(ExpectOp::Eq, Some(json!(1))).evaluate(None).is_err());
    }
}
