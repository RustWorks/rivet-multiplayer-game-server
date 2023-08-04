/// Reads a secret from the env.
///
/// This is marked as async so we have the flexiblity to pull the secret from remote datasources.
pub async fn read_secret(key: &[impl AsRef<str>]) -> Result<String, std::env::VarError> {
	std::env::var(secret_env_var_key(key))
}

pub async fn read_secret_opt(key: &[impl AsRef<str>]) -> Result<Option<String>, std::env::VarError> {
	let env_var = read_secret(key).await;
	
	match env_var {
		Ok(v) => Ok(Some(v)),
		Err(var_error) => {
			match var_error {
				std::env::VarError::NotPresent => Ok(None),
				std::env::VarError::NotUnicode(_) => Err(var_error),
			}
		}
	}
}

/// Name of env var holding a given secret.
pub fn secret_env_var_key(key: &[impl AsRef<str>]) -> String {
	key.iter()
		.map(|x| x.as_ref().to_uppercase())
		.collect::<Vec<_>>()
		.join("_")
}

/// Where this code is being written from. This is derived from the `RIVET_RUN_CONTEXT` environment
/// variable.
///
/// The production run context is not the same as the production namespace.
#[derive(Clone, Debug, PartialEq)]
pub enum RunContext {
	Service,
	Test,
}

impl RunContext {
	fn from_str(ctx: &str) -> Option<RunContext> {
		match ctx {
			"service" => Some(RunContext::Service),
			"test" => Some(RunContext::Test),
			_ => None,
		}
	}
}

pub fn run_context() -> RunContext {
	RUN_CONTEXT.clone().expect("RIVET_RUN_CONTEXT")
}

lazy_static::lazy_static! {
	static ref RUN_CONTEXT: Option<RunContext> = std::env::var("RIVET_RUN_CONTEXT")
		.ok()
		.and_then(|ctx| RunContext::from_str(&ctx));
	static ref NOMAD_REGION: Option<String> = std::env::var("NOMAD_REGION").ok();
	static ref NOMAD_DC: Option<String> = std::env::var("NOMAD_DC").ok();
	static ref REGION: Option<String> = std::env::var("RIVET_REGION").ok();
	static ref NAMESPACE: Option<String> = std::env::var("RIVET_NAMESPACE").ok();
	static ref CLUSTER_ID: Option<String> = std::env::var("RIVET_CLUSTER_ID").ok();
	static ref SOURCE_HASH: Option<String> = std::env::var("RIVET_SOURCE_HASH").ok();
	static ref DOMAIN_MAIN: Option<String> = std::env::var("RIVET_DOMAIN_MAIN").ok();
	static ref DOMAIN_CDN: Option<String> = std::env::var("RIVET_DOMAIN_CDN").ok();
	static ref DOMAIN_JOB: Option<String> = std::env::var("RIVET_DOMAIN_JOB").ok();
	static ref ORIGIN_HUB: Option<String> = std::env::var("RIVET_ORIGIN_HUB").ok();
	static ref PRIMARY_REGION: Option<String> = std::env::var("RIVET_PRIMARY_REGION").ok();
	static ref CHIRP_SERVICE_NAME: Option<String> = std::env::var("CHIRP_SERVICE_NAME").ok();
	static ref IS_BILLING_ENABLED: bool = std::env::var("IS_BILLING_ENABLED")
		.ok()
		.map(|s| s == "1")
		.unwrap_or_default();
}

pub fn nomad_region() -> &'static str {
	match &*NOMAD_REGION {
		Some(x) => x.as_str(),
		None => panic!("NOMAD_REGION"),
	}
}

pub fn nomad_dc() -> &'static str {
	match &*NOMAD_DC {
		Some(x) => x.as_str(),
		None => panic!("NOMAD_DC"),
	}
}

pub fn region() -> &'static str {
	match &*REGION {
		Some(x) => x.as_str(),
		None => panic!("RIVET_REGION"),
	}
}

/// The namespace this service is running in. This is derived from the `NAMESPACE` environment
/// variable.
pub fn namespace() -> &'static str {
	match &*NAMESPACE {
		Some(x) => x.as_str(),
		None => panic!("RIVET_NAMESPACE"),
	}
}

pub fn cluster_id() -> &'static str {
	match &*CLUSTER_ID {
		Some(x) => x.as_str(),
		None => panic!("RIVET_CLUSTER_ID"),
	}
}

/// See `ServiceContextData::source_hash`.
pub fn source_hash() -> &'static str {
	match &*NAMESPACE {
		Some(x) => x.as_str(),
		None => panic!("RIVET_SOURCE_HASH"),
	}
}

/// The base domain in which all subdomains are mounted.
pub fn domain_main() -> &'static str {
	match &*DOMAIN_MAIN {
		Some(x) => x.as_str(),
		None => panic!("RIVET_DOMAIN_MAIN"),
	}
}

/// The base domain in which all game subdomains are mounted.
pub fn domain_cdn() -> &'static str {
	match &*DOMAIN_CDN {
		Some(x) => x.as_str(),
		None => panic!("RIVET_DOMAIN_CDN"),
	}
}

/// The base domain in which all job subdomains are mounted.
pub fn domain_job() -> &'static str {
	match &*DOMAIN_JOB {
		Some(x) => x.as_str(),
		None => panic!("RIVET_DOMAIN_JOB"),
	}
}

/// The base domain for the hub.
pub fn origin_hub() -> &'static str {
	match &*ORIGIN_HUB {
		Some(x) => x.as_str(),
		None => panic!("RIVET_ORIGIN_HUB"),
	}
}

pub fn primary_region() -> &'static str {
	match &*PRIMARY_REGION {
		Some(x) => x.as_str(),
		None => panic!("RIVET_PRIMARY_REGION"),
	}
}

pub fn chirp_service_name() -> &'static str {
	match &*CHIRP_SERVICE_NAME {
		Some(x) => x.as_str(),
		None => panic!("CHIRP_SERVICE_NAME"),
	}
}

pub fn is_billing_enabled() -> bool {
	*IS_BILLING_ENABLED
}

/// Attempts to read a service's public URL from the environment.
pub fn svc_router_url(svc_name: &str) -> String {
	let key = format!("RIVET_{}_URL", svc_name.replace("-", "_").to_uppercase());
	std::env::var(&key).expect(&key)
}

/// The current stripe API token.
pub async fn stripe_token() -> Result<String, std::env::VarError> {
	read_secret(&["stripe", "token"]).await
}

/// The current stripe webhook secret.
///
/// Secrets can be added at: https://dashboard.stripe.com/webhooks
///
/// Add the following events to the WebHook:
/// - invoice.payment_succeeded
/// - checkout.session.completed
/// - payment_intent.succeeded
pub async fn stripe_webhook_secret() -> Result<String, std::env::VarError> {
	read_secret(&["stripe", "webhook_secret"]).await
}

pub mod cloudflare {
	lazy_static::lazy_static! {
		static ref CLOUDFLARE_AUTH_TOKEN: Option<String> = std::env::var("CLOUDFLARE_AUTH_TOKEN").ok();
	}

	pub fn auth_token() -> &'static str {
		match &*CLOUDFLARE_AUTH_TOKEN {
			Some(x) => x.as_str(),
			None => panic!("CLOUDFLARE_AUTH_TOKEN"),
		}
	}

	pub mod zone {
		pub mod base {
			lazy_static::lazy_static! {
				static ref ID: Option<String> = std::env::var("CLOUDFLARE_ZONE_ID_BASE").ok();
			}

			pub fn id() -> &'static str {
				match &*ID {
					Some(x) => x.as_str(),
					None => panic!("CLOUDFLARE_ZONE_ID_BASE"),
				}
			}
		}

		pub mod game {
			lazy_static::lazy_static! {
				static ref ID: Option<String> = std::env::var("CLOUDFLARE_ZONE_ID_GAME").ok();
			}

			pub fn id() -> &'static str {
				match &*ID {
					Some(x) => x.as_str(),
					None => panic!("CLOUDFLARE_ZONE_ID_GAME"),
				}
			}
		}

		pub mod job {
			lazy_static::lazy_static! {
				static ref ID: Option<String> = std::env::var("CLOUDFLARE_ZONE_ID_JOB").ok();
			}

			pub fn id() -> &'static str {
				match &*ID {
					Some(x) => x.as_str(),
					None => panic!("CLOUDFLARE_ZONE_ID_JOB"),
				}
			}
		}
	}
}
