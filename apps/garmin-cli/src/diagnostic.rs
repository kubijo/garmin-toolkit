use anyhow::Error;
use miette::{Diagnostic, MietteHandlerOpts};
use std::fmt::{Debug, Display};
use std::io::IsTerminal as _;

const DEFAULT_REPORT_WIDTH: usize = 80;
const MIN_REPORT_WIDTH: usize = 60;
const MAX_REPORT_WIDTH: usize = 120;

pub fn install(color: bool) -> Result<(), miette::InstallError> {
    let terminal = std::io::stderr().is_terminal();
    let width = crossterm::terminal::size()
        .map_or(DEFAULT_REPORT_WIDTH, |(width, _)| usize::from(width))
        .clamp(MIN_REPORT_WIDTH, MAX_REPORT_WIDTH);
    miette::set_hook(Box::new(move |_| {
        let options = MietteHandlerOpts::new()
            .width(width)
            .color(color)
            .unicode(terminal || color)
            .with_cause_chain();
        if terminal || color {
            Box::new(options.force_graphical(true).build())
        } else {
            Box::new(options.force_narrated(true).build())
        }
    }))
}

pub fn report(error: Error) -> miette::Report {
    miette::Report::new(CliDiagnostic::new(error))
}

struct CliDiagnostic {
    error: Error,
    message: String,
    code: &'static str,
    help: Option<String>,
    include_source: bool,
}

impl CliDiagnostic {
    fn new(error: Error) -> Self {
        let mut diagnostic = Self {
            message: error.to_string(),
            error,
            code: "garmin_cli::operation_failed",
            help: None,
            include_source: true,
        };
        diagnostic.specialize_mtp_failure();
        diagnostic
    }

    fn specialize_mtp_failure(&mut self) {
        let Some(error) = self.error.downcast_ref::<garmin_device::MtpError>() else {
            return;
        };
        match error {
            garmin_device::MtpError::ProbeRecoveryPending { receipt, reason } => {
                "MTP upload recovery is pending".clone_into(&mut self.message);
                self.code = "garmin_cli::mtp::probe_recovery_pending";
                self.help = Some(format!(
                    "The payload may already exist on the device, so no second upload was started. \
                     Recovery state: {}\nRecovery receipt: {}\nRetry the link benchmark with the same device; \
                     the receipt keeps cleanup deterministic.",
                    reason,
                    receipt.display(),
                ));
                self.include_source = false;
            }
            error if error.is_interface_busy() => {
                "The MTP interface is busy".clone_into(&mut self.message);
                self.code = "garmin_cli::mtp::interface_busy";
                self.help = Some(
                    "The operating system reported a busy USB interface, but did not identify its owner. \
                     Wait for the previous session to release it, then retry; use `device list` to confirm \
                     that the Garmin is still present."
                        .to_owned(),
                );
                self.include_source = false;
            }
            _ => {}
        }
    }
}

impl Display for CliDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Debug for CliDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CliDiagnostic")
            .field("message", &self.message)
            .field("code", &self.code)
            .field("help", &self.help)
            .field("include_source", &self.include_source)
            .field("error", &self.error)
            .finish()
    }
}

impl std::error::Error for CliDiagnostic {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.include_source
            .then(|| self.error.chain().nth(1))
            .flatten()
    }
}

impl Diagnostic for CliDiagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        Some(Box::new(self.code))
    }

    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        self.help
            .as_ref()
            .map(|help| Box::new(help) as Box<dyn Display>)
    }
}

#[cfg(test)]
mod tests {
    use super::CliDiagnostic;
    use miette::{GraphicalReportHandler, GraphicalTheme};
    use std::path::PathBuf;

    #[test]
    fn pending_probe_is_structured_and_does_not_guess_the_interface_owner() {
        let error = garmin_device::MtpError::ProbeRecoveryPending {
            receipt: PathBuf::from("/cache/pending/probe.json"),
            reason: garmin_device::ProbeRecoveryReason::InterfaceBusy {
                context: "operation timed out; disconnected-session reconciliation failed"
                    .to_owned(),
            },
        };
        let diagnostic = CliDiagnostic::new(error.into());
        let mut output = String::new();
        GraphicalReportHandler::new_themed(GraphicalTheme::unicode_nocolor())
            .with_width(72)
            .render_report(&mut output, &diagnostic)
            .unwrap();

        assert!(output.contains("MTP upload recovery is pending"));
        assert!(output.contains("garmin_cli::mtp::probe_recovery_pending"));
        assert!(output.contains("/cache/pending/probe.json"));
        assert!(output.contains("owner was not identified"));
        assert!(!output.contains("another process"));
    }
}
