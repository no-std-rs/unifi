// Trusted-main review policy; enrollment and permission requirements: docs/runnerless.md.
import { configuration, event, reviews, checks } from "runnerless/v1";

export function configure(): void {
  configuration.titlePolicy();
  configuration.workflow("codex-review", [
    "pull_request", "pull_request_review", "pull_request_review_comment",
    "pull_request_review_thread", "issue_comment", "check_run",
  ]);
}

export function run(): void {
  const details = event.details();
  // Ignore non-PR and terminal-PR events before evaluating review state.
  if (details.get("pullNumber").isNull() || details.get("state").string() != "open") {
    return;
  }
  const review = reviews.evaluate();
  checks.report("Codex review", review.state == "success", review.summary);
}
