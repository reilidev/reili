//! A small, side-effect-free arithmetic evaluator used to recompute numbers stated by the LLM
//! (error rates, percentages, budget burn). It supports numbers, `+ - * / % ^`, parentheses,
//! unary minus, the constants `pi` and `e`, and a fixed set of pure functions. It never executes
//! code, reads files, or touches the network.

use thiserror::Error;

const MAX_EXPRESSION_LENGTH: usize = 2_000;
const MAX_NESTING_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExpressionError {
    #[error("expression is empty")]
    Empty,
    #[error("expression exceeds {MAX_EXPRESSION_LENGTH} characters")]
    TooLong,
    #[error("expression nesting exceeds {MAX_NESTING_DEPTH} levels")]
    TooDeep,
    #[error("unexpected character `{0}`")]
    UnexpectedCharacter(char),
    #[error("unexpected end of expression")]
    UnexpectedEnd,
    #[error("unexpected token `{0}`")]
    UnexpectedToken(String),
    #[error("unknown identifier `{0}`")]
    UnknownIdentifier(String),
    #[error("function `{name}` expects {expected} argument(s), got {actual}")]
    Arity {
        name: String,
        expected: String,
        actual: usize,
    },
    #[error("result is not a finite number (division by zero or domain error)")]
    NonFinite,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Identifier(String),
    Operator(char),
    LeftParen,
    RightParen,
    Comma,
}

/// Evaluates `expression` and returns a finite result.
pub fn evaluate_expression(expression: &str) -> Result<f64, ExpressionError> {
    if expression.trim().is_empty() {
        return Err(ExpressionError::Empty);
    }
    if expression.len() > MAX_EXPRESSION_LENGTH {
        return Err(ExpressionError::TooLong);
    }
    let tokens = tokenize(expression)?;
    let mut parser = Parser {
        tokens,
        position: 0,
        depth: 0,
    };
    let value = parser.parse_expression()?;
    if let Some(token) = parser.tokens.get(parser.position) {
        return Err(ExpressionError::UnexpectedToken(format!("{token:?}")));
    }
    if value.is_finite() {
        Ok(value)
    } else {
        Err(ExpressionError::NonFinite)
    }
}

fn tokenize(expression: &str) -> Result<Vec<Token>, ExpressionError> {
    let chars: Vec<char> = expression.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if character.is_whitespace() || character == '_' {
            index += 1;
            continue;
        }
        if character.is_ascii_digit() || character == '.' {
            let start = index;
            while index < chars.len()
                && (chars[index].is_ascii_digit() || chars[index] == '.' || chars[index] == '_')
            {
                index += 1;
            }
            if index < chars.len() && (chars[index] == 'e' || chars[index] == 'E') {
                let exponent_start = index;
                index += 1;
                if index < chars.len() && (chars[index] == '+' || chars[index] == '-') {
                    index += 1;
                }
                if index < chars.len() && chars[index].is_ascii_digit() {
                    while index < chars.len() && chars[index].is_ascii_digit() {
                        index += 1;
                    }
                } else {
                    index = exponent_start;
                }
            }
            let literal: String = chars[start..index].iter().filter(|c| **c != '_').collect();
            let number = literal
                .parse::<f64>()
                .map_err(|_| ExpressionError::UnexpectedToken(literal.clone()))?;
            tokens.push(Token::Number(number));
            continue;
        }
        if character.is_ascii_alphabetic() {
            let start = index;
            while index < chars.len() && (chars[index].is_ascii_alphanumeric()) {
                index += 1;
            }
            tokens.push(Token::Identifier(
                chars[start..index]
                    .iter()
                    .collect::<String>()
                    .to_ascii_lowercase(),
            ));
            continue;
        }
        let token = match character {
            '+' | '-' | '*' | '/' | '%' | '^' => Token::Operator(character),
            '(' => Token::LeftParen,
            ')' => Token::RightParen,
            ',' => Token::Comma,
            other => return Err(ExpressionError::UnexpectedCharacter(other)),
        };
        tokens.push(token);
        index += 1;
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.position).cloned();
        self.position += 1;
        token
    }

    fn enter(&mut self) -> Result<(), ExpressionError> {
        self.depth += 1;
        if self.depth > MAX_NESTING_DEPTH {
            return Err(ExpressionError::TooDeep);
        }
        Ok(())
    }

    fn parse_expression(&mut self) -> Result<f64, ExpressionError> {
        self.enter()?;
        let mut value = self.parse_term()?;
        while let Some(Token::Operator(operator @ ('+' | '-'))) = self.peek().cloned() {
            self.position += 1;
            let right = self.parse_term()?;
            value = if operator == '+' {
                value + right
            } else {
                value - right
            };
        }
        self.depth -= 1;
        Ok(value)
    }

    fn parse_term(&mut self) -> Result<f64, ExpressionError> {
        let mut value = self.parse_unary()?;
        while let Some(Token::Operator(operator @ ('*' | '/' | '%'))) = self.peek().cloned() {
            self.position += 1;
            let right = self.parse_unary()?;
            value = match operator {
                '*' => value * right,
                '/' => value / right,
                _ => value % right,
            };
        }
        Ok(value)
    }

    fn parse_unary(&mut self) -> Result<f64, ExpressionError> {
        if let Some(Token::Operator(operator @ ('-' | '+'))) = self.peek().cloned() {
            self.position += 1;
            self.enter()?;
            let value = self.parse_unary()?;
            self.depth -= 1;
            return Ok(if operator == '-' { -value } else { value });
        }
        self.parse_power()
    }

    fn parse_power(&mut self) -> Result<f64, ExpressionError> {
        let base = self.parse_primary()?;
        if let Some(Token::Operator('^')) = self.peek() {
            self.position += 1;
            self.enter()?;
            let exponent = self.parse_unary()?;
            self.depth -= 1;
            return Ok(base.powf(exponent));
        }
        Ok(base)
    }

    fn parse_primary(&mut self) -> Result<f64, ExpressionError> {
        match self.next() {
            Some(Token::Number(value)) => Ok(value),
            Some(Token::LeftParen) => {
                let value = self.parse_expression()?;
                match self.next() {
                    Some(Token::RightParen) => Ok(value),
                    Some(other) => Err(ExpressionError::UnexpectedToken(format!("{other:?}"))),
                    None => Err(ExpressionError::UnexpectedEnd),
                }
            }
            Some(Token::Identifier(name)) => self.parse_identifier(name),
            Some(other) => Err(ExpressionError::UnexpectedToken(format!("{other:?}"))),
            None => Err(ExpressionError::UnexpectedEnd),
        }
    }

    fn parse_identifier(&mut self, name: String) -> Result<f64, ExpressionError> {
        if !matches!(self.peek(), Some(Token::LeftParen)) {
            return match name.as_str() {
                "pi" => Ok(std::f64::consts::PI),
                "e" => Ok(std::f64::consts::E),
                _ => Err(ExpressionError::UnknownIdentifier(name)),
            };
        }
        self.position += 1;
        let mut arguments = Vec::new();
        if !matches!(self.peek(), Some(Token::RightParen)) {
            loop {
                arguments.push(self.parse_expression()?);
                match self.next() {
                    Some(Token::Comma) => {}
                    Some(Token::RightParen) => break,
                    Some(other) => {
                        return Err(ExpressionError::UnexpectedToken(format!("{other:?}")));
                    }
                    None => return Err(ExpressionError::UnexpectedEnd),
                }
            }
        } else {
            self.position += 1;
        }
        apply_function(&name, &arguments)
    }
}

fn apply_function(name: &str, arguments: &[f64]) -> Result<f64, ExpressionError> {
    let unary = |function: fn(f64) -> f64| -> Result<f64, ExpressionError> {
        match arguments {
            [value] => Ok(function(*value)),
            _ => Err(ExpressionError::Arity {
                name: name.to_string(),
                expected: "1".to_string(),
                actual: arguments.len(),
            }),
        }
    };
    match name {
        "abs" => unary(f64::abs),
        "sqrt" => unary(f64::sqrt),
        "ln" => unary(f64::ln),
        "log10" => unary(f64::log10),
        "exp" => unary(f64::exp),
        "floor" => unary(f64::floor),
        "ceil" => unary(f64::ceil),
        "round" => unary(f64::round),
        "pow" => match arguments {
            [base, exponent] => Ok(base.powf(*exponent)),
            _ => Err(ExpressionError::Arity {
                name: name.to_string(),
                expected: "2".to_string(),
                actual: arguments.len(),
            }),
        },
        "min" | "max" | "sum" | "avg" => {
            if arguments.is_empty() {
                return Err(ExpressionError::Arity {
                    name: name.to_string(),
                    expected: "at least 1".to_string(),
                    actual: 0,
                });
            }
            let iter = arguments.iter().copied();
            Ok(match name {
                "min" => iter.fold(f64::INFINITY, f64::min),
                "max" => iter.fold(f64::NEG_INFINITY, f64::max),
                "sum" => iter.sum(),
                _ => {
                    #[allow(clippy::cast_precision_loss)]
                    let count = arguments.len() as f64;
                    iter.sum::<f64>() / count
                }
            })
        }
        _ => Err(ExpressionError::UnknownIdentifier(name.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::{ExpressionError, evaluate_expression};

    #[test]
    fn evaluates_arithmetic_with_precedence() {
        assert_eq!(evaluate_expression("1 + 2 * 3"), Ok(7.0));
        assert_eq!(evaluate_expression("(1 + 2) * 3"), Ok(9.0));
        assert_eq!(evaluate_expression("-2 ^ 2"), Ok(-4.0));
        assert_eq!(evaluate_expression("2 ^ 3 ^ 2"), Ok(512.0));
        assert_eq!(evaluate_expression("10 % 4"), Ok(2.0));
        assert_eq!(evaluate_expression("1_000 / 4"), Ok(250.0));
        assert_eq!(evaluate_expression("1.5e3 + 1"), Ok(1501.0));
    }

    #[test]
    fn evaluates_functions_and_constants() {
        assert_eq!(evaluate_expression("max(1, 5, 3)"), Ok(5.0));
        assert_eq!(evaluate_expression("avg(2, 4)"), Ok(3.0));
        assert_eq!(
            evaluate_expression("round(452 / 120000 * 10000) / 100"),
            Ok(0.38)
        );
        assert!(
            (evaluate_expression("2 * pi").expect("value") - std::f64::consts::TAU).abs() < 1e-12
        );
    }

    #[test]
    fn rejects_invalid_or_unsafe_input() {
        assert_eq!(evaluate_expression(""), Err(ExpressionError::Empty));
        assert_eq!(
            evaluate_expression("1 / 0"),
            Err(ExpressionError::NonFinite)
        );
        assert_eq!(
            evaluate_expression("system(1)"),
            Err(ExpressionError::UnknownIdentifier("system".to_string()))
        );
        assert_eq!(
            evaluate_expression("1; rm"),
            Err(ExpressionError::UnexpectedCharacter(';'))
        );
        assert_eq!(
            evaluate_expression("(1 + 2"),
            Err(ExpressionError::UnexpectedEnd)
        );
        let deep = format!("{}1{}", "(".repeat(100), ")".repeat(100));
        assert_eq!(evaluate_expression(&deep), Err(ExpressionError::TooDeep));
    }
}
