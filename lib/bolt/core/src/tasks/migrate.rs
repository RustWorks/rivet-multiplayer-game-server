use anyhow::*;
use bolt_config::service::RuntimeKind;
use duct::cmd;
use std::time::Duration;
use tokio::task::block_in_place;

use crate::{
	context::{ProjectContext, ServiceContext},
	dep,
	utils::{self, db_conn::DatabaseConnection},
};

pub async fn create(
	_ctx: &ProjectContext,
	service: &ServiceContext,
	migration_name: &str,
) -> Result<()> {
	let db_ext = match &service.config().runtime {
		RuntimeKind::CRDB { .. } => "sql",
		RuntimeKind::ClickHouse { .. } => "sql",
		RuntimeKind::Postgres { .. } => "sql",
		RuntimeKind::Cassandra { .. } => "cql",
		x @ _ => bail!("cannot migrate this type of service: {x:?}"),
	};

	block_in_place(|| {
		cmd!(
			"migrate",
			"create",
			"-ext",
			db_ext,
			"-dir",
			service.migrations_path(),
			migration_name,
		)
		.run()
	})?;

	Ok(())
}

pub async fn check_all(ctx: &ProjectContext) -> Result<()> {
	let services = ctx.services_with_migrations().await;
	check(ctx, &services[..]).await
}

pub async fn check(_ctx: &ProjectContext, services: &[ServiceContext]) -> Result<()> {
	// Spawn Cockroach test container
	let crdb_port = utils::pick_port();
	let crdb_container_id = if services
		.iter()
		.any(|x| matches!(x.config().runtime, RuntimeKind::CRDB { .. }))
	{
		let image = "cockroachdb/cockroach:v22.2.0";
		rivet_term::status::progress("Creating container", image);
		let container_id_bytes = block_in_place(|| {
			cmd!(
				"docker",
				"run",
				"-d",
				"--rm",
				"-p",
				&format!("{crdb_port}:26257"),
				image,
				"start-single-node",
				"--insecure",
			)
			.stdout_capture()
			.run()
		})?
		.stdout;
		let container_id = String::from_utf8(container_id_bytes)?.trim().to_string();

		// Wait for the service to boot
		rivet_term::status::progress("Waiting for database to start", "");
		loop {
			let test_cmd = dep::postgres::cli::exec(
				dep::postgres::cli::Credentials {
					hostname: "127.0.0.1",
					port: crdb_port,
					username: "root",
					password: Some("postgres"),
					db_name: "postgres",
				},
				dep::postgres::cli::Compatability::Cockroach,
				Some("SELECT 1;"),
			)
			.await;
			if test_cmd.is_ok() {
				break;
			}

			tokio::time::sleep(Duration::from_secs(1)).await;
		}

		Some(container_id)
	} else {
		None
	};

	// Spawn ClickHouse test container
	let clickhouse_port = utils::pick_port();
	let clickhouse_container_id = if services
		.iter()
		.any(|x| matches!(x.config().runtime, RuntimeKind::ClickHouse { .. }))
	{
		let image = "clickhouse/clickhouse-server:22.12.3.5-alpine";
		rivet_term::status::progress("Creating container", image);
		let container_id_bytes = block_in_place(|| {
			cmd!(
				"docker",
				"run",
				"-d",
				"--rm",
				"-p",
				&format!("{clickhouse_port}:9000"),
				image,
			)
			.stdout_capture()
			.run()
		})?
		.stdout;
		let container_id = String::from_utf8(container_id_bytes)?.trim().to_string();

		// Wait for the service to boot
		rivet_term::status::progress("Waiting for database to start", "");
		loop {
			let test_cmd = block_in_place(|| {
				cmd!(
					"clickhouse",
					"client",
					"-q",
					"--port",
					clickhouse_port.to_string(),
					"SELECT 1;"
				)
				.stdout_null()
				.stderr_null()
				.run()
			});
			if test_cmd.is_ok() {
				break;
			}

			tokio::time::sleep(Duration::from_secs(1)).await;
		}

		Some(container_id)
	} else {
		None
	};

	// Spawn Postgres test container
	let pg_port = utils::pick_port();
	let pg_container_id = if services
		.iter()
		.any(|x| matches!(x.config().runtime, RuntimeKind::Postgres { .. }))
	{
		let image = "postgres:15.3";
		rivet_term::status::progress("Creating container", image);
		let container_id_bytes = block_in_place(|| {
			cmd!(
				"docker",
				"run",
				"-d",
				"--rm",
				"-p",
				&format!("{pg_port}:26257"),
				image,
				"start-single-node",
				"--insecure",
			)
			.stdout_capture()
			.run()
		})?
		.stdout;
		let container_id = String::from_utf8(container_id_bytes)?.trim().to_string();

		// Wait for the service to boot
		rivet_term::status::progress("Waiting for database to start", "");
		loop {
			let test_cmd = dep::postgres::cli::exec(
				dep::postgres::cli::Credentials {
					hostname: "127.0.0.1",
					port: crdb_port,
					username: "root",
					password: Some("postgres"),
					db_name: "postgres",
				},
				dep::postgres::cli::Compatability::Native,
				Some("SELECT 1;"),
			)
			.await;
			if test_cmd.is_ok() {
				break;
			}

			tokio::time::sleep(Duration::from_secs(1)).await;
		}

		Some(container_id)
	} else {
		None
	};

	// Spawn Cassandra test container
	let cass_container_id = if services
		.iter()
		.any(|x| matches!(x.config().runtime, RuntimeKind::Cassandra { .. }))
	{
		todo!("cassandra not implemented")
	} else {
		Option::<String>::None
	};

	// Run migrations against test containers
	for svc in services {
		eprintln!();
		rivet_term::status::progress("Checking", svc.name());

		let database_url = match &svc.config().runtime {
			RuntimeKind::CRDB { .. } => {
				// Build URL
				let db_name = svc.crdb_db_name();
				let database_url =
					format!("cockroach://root@127.0.0.1:{crdb_port}/{db_name}?sslmode=disable",);

				// Create database
				dep::postgres::cli::exec(
					dep::postgres::cli::Credentials {
						hostname: "127.0.0.1",
						port: crdb_port,
						username: "root",
						password: None,
						db_name: "postgres",
					},
					dep::postgres::cli::Compatability::Cockroach,
					Some(&format!("CREATE DATABASE IF NOT EXISTS \"{db_name}\";")),
				)
				.await?;

				database_url
			}
			RuntimeKind::ClickHouse { .. } => {
				// Build URL
				let db_name = svc.clickhouse_db_name();
				let database_url =
					format!("clickhouse://127.0.0.1:{clickhouse_port}/?database={db_name}&x-multi-statement=true");

				// Create database
				block_in_place(|| {
					cmd!(
						"clickhouse",
						"client",
						"--port",
						clickhouse_port.to_string(),
						"--query",
						format!("CREATE DATABASE IF NOT EXISTS \"{db_name}\";")
					)
					.run()
				})?;

				database_url
			}
			RuntimeKind::Postgres { .. } => {
				// Build URL
				let db_name = svc.pg_db_name();
				let database_url =
					format!("postgres://root@127.0.0.1:{pg_port}/{db_name}?sslmode=disable",);

				// Create database
				dep::postgres::cli::exec(
					dep::postgres::cli::Credentials {
						hostname: "127.0.0.1",
						port: pg_port,
						username: "root",
						password: None,
						db_name: "postgres",
					},
					dep::postgres::cli::Compatability::Native,
					Some(&format!("CREATE DATABASE \"{db_name}\";")),
				)
				.await?;

				database_url
			}
			RuntimeKind::Cassandra {} => {
				todo!()
			}
			x @ _ => bail!("cannot migrate this type of service: {x:?}"),
		};

		block_in_place(|| {
			cmd!(
				"migrate",
				"-database",
				database_url,
				"-path",
				svc.migrations_path(),
				"up",
			)
			.run()
		})?;

		rivet_term::status::success("Migrations valid", "");
	}

	// Kill containers
	println!();
	if let Some(id) = crdb_container_id {
		rivet_term::status::progress("Killing Cockroach container", "");
		block_in_place(|| cmd!("docker", "stop", "-t", "0", id).run())?;
	}
	if let Some(id) = clickhouse_container_id {
		rivet_term::status::progress("Killing ClickHouse container", "");
		block_in_place(|| cmd!("docker", "stop", "-t", "0", id).run())?;
	}
	if let Some(id) = pg_container_id {
		rivet_term::status::progress("Killing Postgres container", "");
		block_in_place(|| cmd!("docker", "stop", "-t", "0", id).run())?;
	}
	if let Some(id) = cass_container_id {
		rivet_term::status::progress("Killing Cassandra container", "");
		block_in_place(|| cmd!("docker", "stop", "-t", "0", id).run())?;
	}

	Ok(())
}

pub async fn up_all(ctx: &ProjectContext) -> Result<()> {
	let services = ctx.services_with_migrations().await;
	up(ctx, &services[..]).await
}

pub async fn up(ctx: &ProjectContext, services: &[ServiceContext]) -> Result<()> {
	let conn = DatabaseConnection::create(ctx, services).await?;

	// Run migrations
	for svc in services {
		let database_url = conn.migrate_db_url(svc).await?;

		eprintln!();

		match &svc.config().runtime {
			RuntimeKind::CRDB { .. } => {
				let db_name = svc.crdb_db_name();
				rivet_term::status::progress("Migrating Cockroach", &db_name);

				let host = conn.cockroach_host.as_ref().unwrap();
				let (hostname, port) = host.split_once(":").unwrap();

				rivet_term::status::progress("Creating database", &db_name);
				dep::postgres::cli::exec(
					dep::postgres::cli::Credentials {
						hostname,
						port: port.parse()?,
						username: "root",
						password: Some("postgres"),
						db_name: "postgres",
					},
					dep::postgres::cli::Compatability::Cockroach,
					Some(&format!("CREATE DATABASE IF NOT EXISTS \"{db_name}\";")),
				)
				.await?;
			}
			RuntimeKind::ClickHouse { .. } => {
				let db_name = svc.clickhouse_db_name();
				rivet_term::status::progress("Migrating ClickHouse", &db_name);

				let clickhouse_user = "bolt";
				let clickhouse_password = ctx
					.read_secret(&["clickhouse", "users", "bolt", "password"])
					.await?;
				let host = conn.clickhouse_host.as_ref().unwrap();
				let (hostname, port) = host.split_once(":").unwrap();

				rivet_term::status::progress("Creating database", &db_name);
				block_in_place(|| {
					cmd!(
						"clickhouse",
						"client",
						"--host",
						hostname,
						"--port",
						port,
						"--user",
						clickhouse_user,
						"--password",
						clickhouse_password,
						"--query",
						format!("CREATE DATABASE IF NOT EXISTS \"{db_name}\";")
					)
					.run()
				})?;
			}
			RuntimeKind::Postgres { .. } => {
				let db_name = svc.crdb_db_name();
				rivet_term::status::progress("Migrating Postgres", &db_name);

				let url = conn.pg_url.as_ref().unwrap();
				let hostname = url.host_str().unwrap();
				let port = url.port().unwrap();
				let username = url.username();
				let password = url.password().unwrap();
				let default_db_name = url.path().trim_start_matches('/');

				// HACK: Ignore error since there is no `CREATE DATABASE IF NOT EXISTS` in Postgres
				rivet_term::status::progress("Creating database", &db_name);
				let _ = dep::postgres::cli::exec(
					dep::postgres::cli::Credentials {
						hostname,
						port,
						username,
						password: Some(password),
						db_name: default_db_name,
					},
					dep::postgres::cli::Compatability::Native,
					Some(&format!("CREATE DATABASE \"{db_name}\";")),
				)
				.await;
			}
			RuntimeKind::Cassandra { .. } => {
				let db_name = svc.crdb_db_name();
				rivet_term::status::progress("Migrating Cassandra", &db_name);

				let url = conn.cass_url.as_ref().unwrap();
				let hostname = url.host_str().unwrap();
				let port = url.port().unwrap();
				let default_keyspace = url.path().trim_start_matches('/');

				// Parse `username` and `password` from query
				let query = url.query().unwrap();
				let username = query
					.split("&")
					.find(|x| x.starts_with("username="))
					.unwrap()
					.trim_start_matches("username=");
				let password = query
					.split("&")
					.find(|x| x.starts_with("password="))
					.unwrap()
					.trim_start_matches("password=");

				// HACK: Ignore error since there is no `CREATE DATABASE IF NOT EXISTS` in Postgres
				rivet_term::status::progress("Creating database", &db_name);
				let replication_factor = svc.cass_replication_factor().await;
				dep::cassandra::cli::exec(
					dep::cassandra::cli::Credentials {
						hostname,
						port,
						username,
						password: Some(password),
						keyspace: default_keyspace,
					},
					Some(&format!("CREATE KEYSPACE IF NOT EXISTS \"{db_name}\" WITH replication = {{'class': 'NetworkTopologyStrategy', 'replication_factor': {replication_factor}}};")),
				)
				.await?;
			}
			x @ _ => bail!("cannot migrate this type of service: {x:?}"),
		}

		rivet_term::status::progress("Running migrations", "");
		block_in_place(|| {
			cmd!(
				"migrate",
				"-database",
				database_url,
				"-path",
				svc.migrations_path(),
				"up",
			)
			.run()
		})?;

		rivet_term::status::success("Migrated", "");
	}

	Ok(())
}

pub async fn down(ctx: &ProjectContext, service: &ServiceContext, num: usize) -> Result<()> {
	let conn = DatabaseConnection::create(ctx, &[service.clone()]).await?;
	let database_url = conn.migrate_db_url(service).await?;

	block_in_place(|| {
		cmd!(
			"migrate",
			"-database",
			database_url,
			"-path",
			service.migrations_path(),
			"down",
			num.to_string(),
		)
		.run()
	})?;

	Ok(())
}

pub async fn force(ctx: &ProjectContext, service: &ServiceContext, num: usize) -> Result<()> {
	let conn = DatabaseConnection::create(ctx, &[service.clone()]).await?;
	let database_url = conn.migrate_db_url(service).await?;

	block_in_place(|| {
		cmd!(
			"migrate",
			"-database",
			database_url,
			"-path",
			service.migrations_path(),
			"force",
			num.to_string(),
		)
		.run()
	})?;

	Ok(())
}

pub async fn drop(ctx: &ProjectContext, service: &ServiceContext) -> Result<()> {
	let conn = DatabaseConnection::create(ctx, &[service.clone()]).await?;
	let database_url = conn.migrate_db_url(service).await?;

	block_in_place(|| {
		cmd!(
			"migrate",
			"-database",
			database_url,
			"-path",
			service.migrations_path(),
			"drop",
		)
		.run()
	})?;

	Ok(())
}
