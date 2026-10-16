# Provider failover and billing consent

An HTTP 402 or pre-output billing-exhaustion error from an OpenAI-compatible profile can offer another configured profile serving the exact same model. The provider does not send the conversation or tools to that other endpoint.

Profile offers are **manual in both `manual` and `countdown` modes**. The notice names the exact command, for example `/model prof-b:glm-5.3`. Select the profile and resend the turn to authorize that request. This applies to local and remote TUI sessions. `jcode run` prints the matching `--model` command and exits without resending.

A three-second cancellable countdown is reserved for an exact, approved auto-switch pool target. A missing target route is a manual offer, not permission to infer an endpoint or bill another profile. When a countdown is allowed, its target is checked again at expiry, so removing or excluding an account prevents the automatic switch.

During a manual profile failover chain, unavailable profiles are remembered briefly. If the selected sibling also fails before output, another matching profile may be offered. Each change still requires an explicit manual selection. Nothing in the countdown setting enrolls metered profiles into automatic billing.
