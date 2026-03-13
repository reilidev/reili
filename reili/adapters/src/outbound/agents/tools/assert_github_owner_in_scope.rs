use reili_shared::errors::PortError;

pub fn assert_github_owner_in_scope(owner: &str, scope_org: &str) -> Result<(), PortError> {
    if owner.eq_ignore_ascii_case(scope_org) {
        return Ok(());
    }

    Err(PortError::new(format!(
        "owner is out of scope. allowed owner: {scope_org}"
    )))
}

#[cfg(test)]
mod tests {
    use super::assert_github_owner_in_scope;

    #[test]
    fn allows_owner_with_case_difference() {
        let result = assert_github_owner_in_scope("Acme", "acme");

        assert!(result.is_ok());
    }

    #[test]
    fn rejects_owner_out_of_scope() {
        let result = assert_github_owner_in_scope("other-org", "acme");

        let error = result.expect_err("owner should be rejected");
        assert_eq!(error.message, "owner is out of scope. allowed owner: acme");
    }
}
