// Windows service dispatcher smoke tests.
//
// These tests only run on Windows — on Linux/macOS the cfg gate skips the
// entire file so the test binary still compiles and links cleanly.
//
// Real end-to-end validation (sc.exe create → Start-Service → Stop-Service)
// requires a Windows VM; see docs/wave4/09-agent-service-dispatcher.md for
// the manual verification procedure.

#[cfg(windows)]
mod windows_only {
    use cmtraceopen_agent::service::SERVICE_NAME;

    /// The service name must match what the MSI installer registers.
    /// If this changes, the installer sources must be updated in lockstep.
    #[test]
    fn service_name_is_stable() {
        assert_eq!(SERVICE_NAME, "CMTraceOpenAgent");
    }

    /// Calling `try_run_as_service` from a plain test process (not under the
    /// SCM) must return `None` — indicating that CLI fall-through is correct.
    /// A `Some(_)` here would mean either the binary is inexplicably running
    /// under the SCM during tests, or the error-code detection is broken.
    #[test]
    fn try_run_as_service_returns_none_outside_scm() {
        // This will call `service_dispatcher::start` which returns
        // ERROR_FAILED_SERVICE_CONTROLLER_CONNECT (1063) when not invoked by
        // the SCM. Our wrapper must map that to `None`.
        let result = cmtraceopen_agent::service::try_run_as_service();
        assert!(
            result.is_none(),
            "expected None (not-under-SCM) but got Some({result:?})"
        );
    }
}
