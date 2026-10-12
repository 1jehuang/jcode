#[path = "e2e/mock_provider.rs"]
mod mock_provider;

// The shared support module also serves the larger e2e binary. This target
// only runs the provider-behavior cases, so the rest of the helpers are unused.
#[allow(unused)]
#[path = "e2e/test_support/mod.rs"]
mod test_support;

#[path = "e2e/provider_behavior.rs"]
mod provider_behavior;
