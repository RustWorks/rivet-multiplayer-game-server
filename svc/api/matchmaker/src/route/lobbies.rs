use api_helper::{anchor::WatchIndexQuery, ctx::Ctx};
use proto::{
	backend::{self, pkg::*},
	common,
};
use rivet_api::models;
use rivet_convert::ApiTryInto;
use rivet_operation::prelude::*;
use serde_json::json;
use std::{
	collections::{HashMap, HashSet},
	str::FromStr,
};

use crate::{
	auth::Auth,
	fetch::game::{fetch_ns, NamespaceData},
	utils,
};

// MARK: POST /lobbies/ready
pub async fn ready(ctx: Ctx<Auth>, _body: serde_json::Value) -> GlobalResult<serde_json::Value> {
	// Mock response
	if ctx.auth().game_ns_dev_option()?.is_some() {
		return Ok(json!({}));
	}

	let lobby_ent = ctx.auth().lobby()?;

	msg!([ctx] mm::msg::lobby_ready(lobby_ent.lobby_id) {
		lobby_id: Some(lobby_ent.lobby_id.into()),
	})
	.await?;

	Ok(json!({}))
}

// MARK: POST /lobbies/join
pub async fn join(
	ctx: Ctx<Auth>,
	body: models::MatchmakerLobbiesJoinRequest,
) -> GlobalResult<models::MatchmakerJoinLobbyResponse> {
	// Mock response
	if let Some(ns_dev_ent) = ctx.auth().game_ns_dev_option()? {
		let FindResponse {
			lobby,
			ports,
			player,
		} = dev_mock_lobby(&ctx, &ns_dev_ent).await?;
		return Ok(models::MatchmakerJoinLobbyResponse {
			lobby,
			ports,
			player,
		});
	}

	let game_ns = ctx.auth().game_ns(&ctx).await?;
	let ns_data = fetch_ns(&ctx, &game_ns).await?;
	let lobby_id = Uuid::from_str(body.lobby_id.as_str())?;

	let find_query =
		mm::msg::lobby_find::message::Query::Direct(backend::matchmaker::query::Direct {
			lobby_id: Some(lobby_id.into()),
		});
	let FindResponse {
		lobby,
		ports,
		player,
	} = find_inner(&ctx, &ns_data, find_query, body.captcha).await?;

	Ok(models::MatchmakerJoinLobbyResponse {
		lobby,
		ports,
		player,
	})
}

// MARK: POST /lobbies/find
pub async fn find(
	ctx: Ctx<Auth>,
	body: models::MatchmakerLobbiesFindRequest,
) -> GlobalResult<models::MatchmakerFindLobbyResponse> {
	let (lat, long) = internal_unwrap_owned!(ctx.coords());

	// Mock response
	if let Some(ns_dev_ent) = ctx.auth().game_ns_dev_option()? {
		let FindResponse {
			lobby,
			ports,
			player,
		} = dev_mock_lobby(&ctx, &ns_dev_ent).await?;
		return Ok(models::MatchmakerFindLobbyResponse {
			lobby,
			ports,
			player,
		});
	}

	let game_ns = ctx.auth().game_ns(&ctx).await?;
	let ns_data = fetch_ns(&ctx, &game_ns).await?;

	// Fetch version data
	let version_res = op!([ctx] mm_config_version_get {
		version_ids: vec![ns_data.version_id.into()],
	})
	.await?;
	let version_data = internal_unwrap_owned!(version_res.versions.first());
	let version_config = internal_unwrap!(version_data.config);
	let version_meta = internal_unwrap!(version_data.config_meta);

	// Find lobby groups that match the requested game modes. This matches the
	// same order as `body.game_modes`.
	let lobby_groups: Vec<(
		&backend::matchmaker::LobbyGroup,
		&backend::matchmaker::LobbyGroupMeta,
	)> = body
		.game_modes
		.iter()
		.map(|name_id| {
			Ok(unwrap_with_owned!(
				version_config
					.lobby_groups
					.iter()
					.zip(version_meta.lobby_groups.iter())
					.find(|(lgc, _)| lgc.name_id == *name_id),
				MATCHMAKER_GAME_MODE_NOT_FOUND
			))
		})
		.collect::<GlobalResult<Vec<_>>>()?;

	// Resolve the region IDs.
	//
	// `region_ids` represents the requested regions in order of priority.
	let region_ids = if let Some(region_name_ids) = body.regions {
		// Resolve the region ID corresponding to the name IDs
		let resolve_res = op!([ctx] region_resolve {
			name_ids: region_name_ids.clone(),
		})
		.await?;

		// Map to region IDs and decide
		let region_ids = region_name_ids
			.iter()
			.flat_map(|name_id| resolve_res.regions.iter().find(|r| r.name_id == *name_id))
			.flat_map(|r| r.region_id.as_ref())
			.map(common::Uuid::as_uuid)
			.collect::<Vec<_>>();

		internal_assert_eq!(region_ids.len(), region_name_ids.len(), "region not found");

		region_ids
	} else {
		// Find all enabled region IDs in all requested lobby groups
		let enabled_region_ids = lobby_groups
			.iter()
			.flat_map(|(lg, _)| {
				lg.regions
					.iter()
					.filter_map(|r| r.region_id.as_ref())
					.map(common::Uuid::as_uuid)
					.collect::<Vec<_>>()
			})
			.collect::<HashSet<Uuid>>()
			.into_iter()
			.map(Into::<common::Uuid>::into)
			.collect::<Vec<_>>();

		// Auto-select the closest region
		let recommend_res = op!([ctx] region_recommend {
			latitude: Some(lat),
			longitude: Some(long),
			region_ids: enabled_region_ids,
			..Default::default()
		})
		.await?;
		let primary_region = internal_unwrap_owned!(recommend_res.regions.first());
		let primary_region_id = internal_unwrap!(primary_region.region_id).as_uuid();

		vec![primary_region_id]
	};

	// Validate that there is a lobby group and region pair that is valid.
	//
	// We also derive the auto create config at the same time, since the
	// auto-create config is the first pair of lobby group and regions that are
	// valid.
	//
	// If an auto-create configuration can't be derived, then there's also no
	// existing lobbies that can exist.
	let mut auto_create = None;
	'lg: for (lgc, lgm) in &lobby_groups {
		// Parse the region IDs for the lobby group
		let lobby_group_region_ids = lgc
			.regions
			.iter()
			.filter_map(|x| x.region_id.as_ref())
			.map(common::Uuid::as_uuid)
			.collect::<Vec<_>>();

		// Find the first region that matches this lobby group
		if let Some(region_id) = region_ids
			.iter()
			.find(|region_id| lobby_group_region_ids.contains(region_id))
		{
			auto_create = Some(backend::matchmaker::query::AutoCreate {
				lobby_group_id: lgm.lobby_group_id,
				region_id: Some((*region_id).into()),
			});
			break 'lg;
		}

		tracing::info!(
			?lgc,
			?lobby_group_region_ids,
			"no regions match the lobby group"
		);
	}

	// Unwrap the auto-create value
	let auto_create = if let Some(auto_create) = auto_create {
		auto_create
	} else {
		internal_panic!("no valid lobby group and region id pair found for auto-create");
	};

	// Build query and find lobby
	let find_query =
		mm::msg::lobby_find::message::Query::LobbyGroup(backend::matchmaker::query::LobbyGroup {
			lobby_group_ids: lobby_groups
				.iter()
				.filter_map(|(_, lgm)| lgm.lobby_group_id)
				.collect(),
			region_ids: region_ids
				.iter()
				.cloned()
				.map(Into::<common::Uuid>::into)
				.collect(),
			auto_create: if body.prevent_auto_create_lobby == Some(true) {
				None
			} else {
				Some(auto_create)
			},
		});
	let FindResponse {
		lobby,
		ports,
		player,
	} = find_inner(&ctx, &ns_data, find_query, body.captcha).await?;

	Ok(models::MatchmakerFindLobbyResponse {
		lobby,
		ports,
		player,
	})
}

// MARK: GET /lobbies/list
pub async fn list(
	ctx: Ctx<Auth>,
	_watch_index: WatchIndexQuery,
) -> GlobalResult<models::MatchmakerListLobbiesResponse> {
	let (lat, long) = internal_unwrap_owned!(ctx.coords());

	// Mock response
	if let Some(ns_dev_ent) = ctx.auth().game_ns_dev_option()? {
		return dev_mock_lobby_list(&ctx, &ns_dev_ent).await;
	}

	let game_ns = ctx.auth().game_ns(&ctx).await?;

	// TODO: Cache this

	// Fetch version config and lobbies
	let (meta, lobbies) = tokio::try_join!(
		fetch_lobby_list_meta(ctx.op_ctx(), game_ns.namespace_id, lat, long),
		fetch_lobby_list(ctx.op_ctx(), game_ns.namespace_id),
	)?;

	let regions = meta
		.regions
		.iter()
		.map(|(region, recommend)| utils::build_region_openapi(region, recommend))
		.collect();

	let game_modes = meta
		.lobby_groups
		.iter()
		.map(|(gm, _)| models::MatchmakerGameModeInfo {
			game_mode_id: gm.name_id.clone(),
		})
		.collect();

	let lobbies = lobbies
		.iter()
		// Join with lobby group
		.filter_map(|lobby| {
			if let Some((lobby_group, _)) = meta
				.lobby_groups
				.iter()
				.find(|(_, lg)| lg.lobby_group_id == lobby.lobby.lobby_group_id)
			{
				Some((lobby, lobby_group))
			} else {
				// Lobby is outdated
				None
			}
		})
		// Filter out empty lobbies
		.filter(|(lobby, _)| {
			// Keep if lobby not empty
			if lobby.player_count.registered_player_count != 0 {
				return true;
			}

			// Keep if this is the only lobby in this lobby group
			if lobbies
				.iter()
				.filter(|x| x.lobby.lobby_group_id == lobby.lobby.lobby_group_id)
				.count() == 1
			{
				return true;
			}

			// This lobby is empty (i.e. idle) and should not be listed
			false
		})
		// Build response model
		.map(|(lobby, lobby_group)| {
			let (region, _) = internal_unwrap_owned!(meta
				.regions
				.iter()
				.find(|(r, _)| r.region_id == lobby.lobby.region_id));

			GlobalResult::Ok(models::MatchmakerLobbyInfo {
				region_id: region.name_id.clone(),
				game_mode_id: lobby_group.name_id.clone(),
				lobby_id: internal_unwrap!(lobby.lobby.lobby_id).as_uuid(),
				max_players_normal: std::convert::TryInto::try_into(
					lobby.lobby.max_players_normal,
				)?,
				max_players_direct: std::convert::TryInto::try_into(
					lobby.lobby.max_players_direct,
				)?,
				max_players_party: std::convert::TryInto::try_into(lobby.lobby.max_players_party)?,
				total_player_count: std::convert::TryInto::try_into(
					lobby.player_count.registered_player_count,
				)?,
			})
		})
		.collect::<GlobalResult<Vec<_>>>()?;

	Ok(models::MatchmakerListLobbiesResponse {
		game_modes,
		regions,
		lobbies,
	})
}

async fn dev_mock_lobby_list(
	ctx: &Ctx<Auth>,
	ns_dev_ent: &rivet_claims::ent::GameNamespaceDevelopment,
) -> GlobalResult<models::MatchmakerListLobbiesResponse> {
	// Read the version config
	let ns_res = op!([ctx] game_namespace_get {
		namespace_ids: vec![ns_dev_ent.namespace_id.into()],
	})
	.await?;
	let ns_data = internal_unwrap_owned!(ns_res.namespaces.first());
	let version_id = internal_unwrap!(ns_data.version_id).as_uuid();

	let version_res = op!([ctx] mm_config_version_get {
		version_ids: vec![version_id.into()],
	})
	.await?;
	let version = internal_unwrap_owned!(
		version_res.versions.first(),
		"no matchmaker config for namespace"
	);
	let version_config = internal_unwrap!(version.config);

	// Create fake region
	let region = models::MatchmakerRegionInfo {
		region_id: util_mm::consts::DEV_REGION_ID.into(),
		provider_display_name: util_mm::consts::DEV_PROVIDER_NAME.into(),
		region_display_name: util_mm::consts::DEV_REGION_NAME.into(),
		datacenter_coord: Box::new(models::GeoCoord {
			latitude: 0.0,
			longitude: 0.0,
		}),
		datacenter_distance_from_client: Box::new(models::GeoDistance {
			kilometers: 0.0,
			miles: 0.0,
		}),
	};

	// List game modes
	let game_modes = version_config
		.lobby_groups
		.iter()
		.map(|lg| models::MatchmakerGameModeInfo {
			game_mode_id: lg.name_id.clone(),
		})
		.collect();

	// Create a fake lobby in each game mode
	let lobbies = version_config
		.lobby_groups
		.iter()
		.map(|lg| {
			GlobalResult::Ok(models::MatchmakerLobbyInfo {
				region_id: util_mm::consts::DEV_REGION_ID.into(),
				game_mode_id: lg.name_id.clone(),
				lobby_id: Uuid::nil(),
				max_players_normal: std::convert::TryInto::try_into(lg.max_players_normal)?,
				max_players_direct: std::convert::TryInto::try_into(lg.max_players_direct)?,
				max_players_party: std::convert::TryInto::try_into(lg.max_players_party)?,
				total_player_count: 0,
			})
		})
		.collect::<GlobalResult<Vec<_>>>()?;

	Ok(models::MatchmakerListLobbiesResponse {
		regions: vec![region],
		game_modes,
		lobbies,
	})
}

struct FetchLobbyListMeta {
	lobby_groups: Vec<(
		backend::matchmaker::LobbyGroup,
		backend::matchmaker::LobbyGroupMeta,
	)>,
	regions: Vec<(backend::region::Region, region::recommend::response::Region)>,
}

/// Fetches lobby group & region data in order to build the lobby list response.
async fn fetch_lobby_list_meta(
	ctx: &OperationContext<()>,
	namespace_id: Uuid,
	lat: f64,
	long: f64,
) -> GlobalResult<FetchLobbyListMeta> {
	let ns_res = op!([ctx] game_namespace_get {
		namespace_ids: vec![namespace_id.into()],
	})
	.await?;
	let ns_data = internal_unwrap_owned!(ns_res.namespaces.first());
	let version_id = internal_unwrap!(ns_data.version_id).as_uuid();

	// Read the version config
	let version_res = op!([ctx] mm_config_version_get {
		version_ids: vec![version_id.into()],
	})
	.await?;
	let version = internal_unwrap_owned!(
		version_res.versions.first(),
		"no matchmaker config for namespace"
	);
	let version_config = internal_unwrap!(version.config);
	let version_meta = internal_unwrap!(version.config_meta);
	let lobby_groups = version_config
		.lobby_groups
		.iter()
		.cloned()
		.zip(version_meta.lobby_groups.iter().cloned())
		.collect::<Vec<_>>();

	// Fetch all regions
	let region_ids = version_config
		.lobby_groups
		.iter()
		.flat_map(|lg| lg.regions.iter())
		.filter_map(|r| r.region_id.as_ref())
		.map(common::Uuid::as_uuid)
		.collect::<HashSet<Uuid>>();
	let region_ids_proto = region_ids
		.iter()
		.cloned()
		.map(Into::<common::Uuid>::into)
		.collect::<Vec<_>>();
	let (region_res, recommend_res) = tokio::try_join!(
		op!([ctx] region_get {
			region_ids: region_ids_proto.clone(),
		}),
		op!([ctx] region_recommend {
			region_ids: region_ids_proto.clone(),
			latitude: Some(lat),
			longitude: Some(long),
			..Default::default()
		}),
	)?;

	Ok(FetchLobbyListMeta {
		lobby_groups,
		regions: region_res
			.regions
			.iter()
			.map(|region| {
				let recommend_region = internal_unwrap_owned!(recommend_res
					.regions
					.iter()
					.find(|r| r.region_id == region.region_id));
				GlobalResult::Ok((region.clone(), recommend_region.clone()))
			})
			.collect::<GlobalResult<Vec<_>>>()?,
	})
}

struct FetchLobbyListEntry {
	lobby: backend::matchmaker::Lobby,
	player_count: mm::lobby_player_count::response::Lobby,
}

/// Fetches all the lobbies and their associated player counts.
async fn fetch_lobby_list(
	ctx: &OperationContext<()>,
	namespace_id: Uuid,
) -> GlobalResult<Vec<FetchLobbyListEntry>> {
	// Fetch lobby IDs
	let lobby_ids = {
		let lobby_list_res = op!([ctx] mm_lobby_list_for_namespace {
			namespace_ids: vec![namespace_id.into()],
		})
		.await?;
		let lobby_ids = internal_unwrap_owned!(lobby_list_res.namespaces.first());

		lobby_ids.lobby_ids.clone()
	};

	// Fetch all lobbies
	let lobbies = {
		let (lobby_get_res, player_count_res) = tokio::try_join!(
			op!([ctx] mm_lobby_get {
				lobby_ids: lobby_ids.clone(),
				include_stopped: false,
			}),
			op!([ctx] mm_lobby_player_count {
				lobby_ids: lobby_ids.clone(),
			}),
		)?;

		// Match lobby data with player counts
		lobby_get_res
			.lobbies
			.iter()
			.filter_map(|lobby| {
				player_count_res
					.lobbies
					.iter()
					.find(|pc| pc.lobby_id == lobby.lobby_id)
					.map(|pc| FetchLobbyListEntry {
						lobby: lobby.clone(),
						player_count: pc.clone(),
					})
			})
			.collect::<Vec<_>>()
	};

	Ok(lobbies)
}

// MARK: PUT /lobbies/closed
pub async fn closed(
	ctx: Ctx<Auth>,
	body: models::MatchmakerLobbiesSetClosedRequest,
) -> GlobalResult<serde_json::Value> {
	// Mock response
	if ctx.auth().game_ns_dev_option()?.is_some() {
		return Ok(json!({}));
	}

	let lobby_ent = ctx.auth().lobby()?;

	msg!([ctx] mm::msg::lobby_closed_set(lobby_ent.lobby_id) {
		lobby_id: Some(lobby_ent.lobby_id.into()),
		is_closed: body.is_closed,
	})
	.await?;

	Ok(json!({}))
}

// MARK: Utilities
struct FindResponse {
	lobby: Box<models::MatchmakerJoinLobby>,
	ports: HashMap<String, models::MatchmakerJoinPort>,
	player: Box<models::MatchmakerJoinPlayer>,
}

#[tracing::instrument(err, skip(ctx, game_ns))]
async fn find_inner(
	ctx: &Ctx<Auth>,
	game_ns: &NamespaceData,
	query: mm::msg::lobby_find::message::Query,
	captcha: Option<Box<models::CaptchaConfig>>,
) -> GlobalResult<FindResponse> {
	// Get version config
	let version_config_res = op!([ctx] mm_config_version_get {
		version_ids: vec![game_ns.version_id.into()],
	})
	.await?;

	let version_config = internal_unwrap_owned!(version_config_res.versions.first());
	let version_config = internal_unwrap!(version_config.config);

	// Validate captcha
	if let Some(captcha_config) = &version_config.captcha {
		if let Some(captcha) = captcha {
			// Will throw an error if the captcha is invalid
			op!([ctx] captcha_verify {
				topic: HashMap::<String, String>::from([
					("kind".into(), "mm:find".into()),
				]),
				remote_address: internal_unwrap!(ctx.remote_address()).to_string(),
				origin_host: ctx
					.origin()
					.and_then(|origin| origin.host_str())
					.map(ToString::to_string),
				captcha_config: Some(captcha_config.clone()),
				client_response: Some((*captcha).try_into()?),
				namespace_id: Some(game_ns.namespace_id.into()),
			})
			.await?;
		} else {
			let required_res = op!([ctx] captcha_request {
				topic: HashMap::<String, String>::from([
					("kind".into(), "mm:find".into()),
				]),
				captcha_config: Some(captcha_config.clone()),
				remote_address: internal_unwrap!(ctx.remote_address()).to_string(),
				namespace_id: Some(game_ns.namespace_id.into()),
			})
			.await?;

			if let Some(hcaptcha_config) = &captcha_config.hcaptcha {
				let hcaptcha_config_res = op!([ctx] captcha_hcaptcha_config_get {
					config: Some(hcaptcha_config.clone()),
				})
				.await?;

				assert_with!(
					!required_res.needs_verification,
					CAPTCHA_CAPTCHA_REQUIRED {
						metadata: json!({
							"hcaptcha": {
								"site_id": hcaptcha_config_res.site_key,
							}
						}),
					}
				);
			} else if let Some(_turnstile_config) = &captcha_config.turnstile {
				assert_with!(
					!required_res.needs_verification,
					CAPTCHA_CAPTCHA_REQUIRED {
						metadata: json!({
							"turnstile": {}
						}),
					}
				);
			} else {
				internal_panic!("invalid captcha config for version");
			}
		}
	}

	// Create token
	let player_id = Uuid::new_v4();
	let token_res = op!([ctx] token_create {
		issuer: "api-matchmaker".into(),
		token_config: Some(token::create::request::TokenConfig {
			// Has to be greater than the player register time since this
			// token is used in the player disconnect too.
			ttl: util::duration::days(90),
		}),
		refresh_token_config: None,
		client: Some(ctx.client_info()),
		kind: Some(token::create::request::Kind::New(token::create::request::KindNew {
			entitlements: vec![
				proto::claims::Entitlement {
					kind: Some(
					  proto::claims::entitlement::Kind::MatchmakerPlayer(proto::claims::entitlement::MatchmakerPlayer {
						  player_id: Some(player_id.into()),
					  })
				  )
				}
			],
		})),
		label: Some("player".into()),
		..Default::default()
	})
	.await?;
	let token = internal_unwrap!(token_res.token);
	let token_session_id = internal_unwrap!(token_res.session_id).as_uuid();

	// Find lobby
	let query_id = Uuid::new_v4();
	let find_res = msg!([ctx] @notrace mm::msg::lobby_find(game_ns.namespace_id, query_id) -> Result<mm::msg::lobby_find_complete, mm::msg::lobby_find_fail> {
		namespace_id: Some(game_ns.namespace_id.into()),
		query_id: Some(query_id.into()),
		join_kind: backend::matchmaker::query::JoinKind::Normal as i32,
		players: vec![mm::msg::lobby_find::Player {
			player_id: Some(player_id.into()),
			token_session_id: Some(token_session_id.into()),
			client_info: Some(ctx.client_info()),
		}],
		query: Some(query),
	})
	.await?;
	let lobby_id = match find_res
		.map_err(|msg| mm::msg::lobby_find_fail::ErrorCode::from_i32(msg.error_code))
	{
		Ok(res) => internal_unwrap!(res.lobby_id).as_uuid(),
		Err(Some(code)) => {
			use mm::msg::lobby_find_fail::ErrorCode::*;

			match code {
				Unknown => internal_panic!("unknown find error code"),
				StaleMessage => panic_with!(CHIRP_STALE_MESSAGE),
				TooManyPlayersFromSource => panic_with!(MATCHMAKER_TOO_MANY_PLAYERS_FROM_SOURCE),

				LobbyStopped | LobbyStoppedPrematurely => panic_with!(MATCHMAKER_LOBBY_STOPPED),
				LobbyClosed => panic_with!(MATCHMAKER_LOBBY_CLOSED),
				LobbyNotFound => panic_with!(MATCHMAKER_LOBBY_NOT_FOUND),
				NoAvailableLobbies => panic_with!(MATCHMAKER_NO_AVAILABLE_LOBBIES),
				LobbyFull => panic_with!(MATCHMAKER_LOBBY_FULL),
				LobbyCountOverMax => panic_with!(MATCHMAKER_TOO_MANY_LOBBIES),
				RegionNotEnabled => panic_with!(MATCHMAKER_REGION_NOT_ENABLED_FOR_GAME_MODE),

				DevTeamInvalidStatus => panic_with!(GROUP_INVALID_DEVELOPER_STATUS),
			};
		}
		Err(None) => internal_panic!("failed to parse find error code"),
	};

	// Fetch lobby data
	let lobby_res = op!([ctx] mm_lobby_get {
		lobby_ids: vec![lobby_id.into()],
		..Default::default()
	})
	.await?;
	let lobby = if let Some(lobby) = lobby_res.lobbies.first() {
		lobby
	} else {
		// We should never reach this point, since we preemptively create
		// players in mm-lobby-find which will ensure the lobby is not removed.
		//
		// This will only happen if the lobby manually stops/exits in the middle
		// of a find request.
		tracing::error!("lobby not found in race condition");
		internal_panic!("lobby not found");
	};
	let region_id = internal_unwrap!(lobby.region_id);
	let lobby_group_id = internal_unwrap!(lobby.lobby_group_id);
	let run_id = internal_unwrap!(lobby.run_id);

	// Fetch lobby run data
	let (run_res, version) = tokio::try_join!(
		// Fetch the job run
		async {
			op!([ctx] job_run_get {
				run_ids: vec![*run_id],
			})
			.await
			.map_err(Into::<GlobalError>::into)
		},
		// Fetch the version
		async {
			let version_res = op!([ctx] mm_config_lobby_group_resolve_version {
				lobby_group_ids: vec![*lobby_group_id],
			})
			.await?;

			let version_id = internal_unwrap_owned!(version_res.versions.first());
			let version_id = internal_unwrap!(version_id.version_id);
			let version_res = op!([ctx] mm_config_version_get {
				version_ids: vec![*version_id],
			})
			.await?;
			let version = internal_unwrap_owned!(version_res.versions.first());

			GlobalResult::Ok(version.clone())
		}
	)?;

	// Match the version
	let version_config = internal_unwrap!(version.config);
	let version_meta = internal_unwrap!(version.config_meta);
	let (lobby_group_config, _lobby_group_meta) = internal_unwrap_owned!(version_config
		.lobby_groups
		.iter()
		.zip(version_meta.lobby_groups.iter())
		.find(|(_, meta)| meta.lobby_group_id.as_ref() == Some(lobby_group_id)));
	let lobby_runtime = internal_unwrap!(lobby_group_config.runtime);
	#[allow(clippy::infallible_destructuring_match)]
	let docker_runtime = match internal_unwrap!(lobby_runtime.runtime) {
		backend::matchmaker::lobby_runtime::Runtime::Docker(x) => x,
	};

	// Convert the ports to client-friendly ports
	let run = internal_unwrap_owned!(run_res.runs.first());

	let ports = docker_runtime
		.ports
		.iter()
		.map(|port| build_port(run, port))
		.filter_map(|x| x.transpose())
		.collect::<GlobalResult<HashMap<_, _>>>()?;

	// Fetch region data
	let region_res = op!([ctx] region_get {
		region_ids: vec![*region_id],
	})
	.await?;
	let region_proto = internal_unwrap_owned!(region_res.regions.first());
	let region = Box::new(models::MatchmakerJoinRegion {
		region_id: region_proto.name_id.clone(),
		display_name: region_proto.region_display_name.clone(),
	});

	let player = Box::new(models::MatchmakerJoinPlayer {
		token: token.token.clone(),
	});

	// TODO: Gracefully catch errors from this

	// Also see svc/api-identity/src/route/events.rs for fetching the lobby
	Ok(FindResponse {
		lobby: Box::new(models::MatchmakerJoinLobby {
			lobby_id,
			region,
			ports: ports.clone(),
			player: player.clone(),
		}),
		ports,
		player,
	})
}

#[tracing::instrument(err, skip(ctx))]
async fn dev_mock_lobby(
	ctx: &Ctx<Auth>,
	ns_dev_ent: &rivet_claims::ent::GameNamespaceDevelopment,
) -> GlobalResult<FindResponse> {
	// Issue development player
	let player_id = Uuid::new_v4();
	let token = op!([ctx] mm_dev_player_token_create {
		namespace_id: Some(ns_dev_ent.namespace_id.into()),
		player_id: Some(player_id.into()),
	})
	.await?;

	// Find the port to connect to
	let ports = ns_dev_ent
		.lobby_ports
		.iter()
		.map(|port| {
			GlobalResult::Ok((
				port.label.clone(),
				models::MatchmakerJoinPort {
					host: port
						.target_port
						.map(|port| format!("{}:{port}", ns_dev_ent.hostname)),
					hostname: ns_dev_ent.hostname.clone(),
					port: port.target_port.map(|x| x.try_into()).transpose()?,
					port_range: port
						.port_range
						.as_ref()
						.map(|x| {
							GlobalResult::Ok(models::MatchmakerJoinPortRange {
								min: x.min.try_into()?,
								max: x.max.try_into()?,
							})
						})
						.transpose()?
						.map(Box::new),
					is_tls: matches!(
						port.proxy_protocol,
						rivet_claims::ent::DevelopmentProxyProtocol::Https
							| rivet_claims::ent::DevelopmentProxyProtocol::TcpTls
					),
				},
			))
		})
		.collect::<GlobalResult<HashMap<_, _>>>()?;

	let player = Box::new(models::MatchmakerJoinPlayer {
		token: token.player_jwt,
	});

	Ok(FindResponse {
		lobby: Box::new(models::MatchmakerJoinLobby {
			lobby_id: Uuid::nil(),
			region: Box::new(models::MatchmakerJoinRegion {
				region_id: "dev-lcl".into(),
				display_name: "Local".into(),
			}),
			ports: ports.clone(),
			player: player.clone(),
		}),
		ports,
		player,
	})
}

// TODO: Copied to api-identity
fn build_port(
	run: &backend::job::Run,
	port: &backend::matchmaker::lobby_runtime::Port,
) -> GlobalResult<Option<(String, models::MatchmakerJoinPort)>> {
	use backend::job::ProxyProtocol as JobProxyProtocol;
	use backend::matchmaker::lobby_runtime::{
		ProxyKind as MmProxyKind, ProxyProtocol as MmProxyProtocol,
	};

	let proxy_kind = internal_unwrap_owned!(MmProxyKind::from_i32(port.proxy_kind));
	let mm_proxy_protocol = internal_unwrap_owned!(MmProxyProtocol::from_i32(port.proxy_protocol));

	let join_info_port = match (proxy_kind, mm_proxy_protocol) {
		(
			MmProxyKind::GameGuard,
			MmProxyProtocol::Http
			| MmProxyProtocol::Https
			| MmProxyProtocol::Tcp
			| MmProxyProtocol::TcpTls
			| MmProxyProtocol::Udp,
		) => {
			run.proxied_ports
				.iter()
				// Decode the proxy protocol
				.filter_map(|proxied_port| {
					match JobProxyProtocol::from_i32(proxied_port.proxy_protocol) {
						Some(x) => Some((proxied_port, x)),
						None => {
							tracing::error!(?proxied_port, "could not decode job proxy protocol");
							None
						}
					}
				})
				// Match the matchmaker port with the job port that matches the same
				// port and protocol
				.filter(|(proxied_port, job_proxy_protocol)| {
					test_mm_and_job_proxy_protocol_eq(mm_proxy_protocol, *job_proxy_protocol)
						&& proxied_port.target_nomad_port_label
							== Some(util_mm::format_nomad_port_label(&port.label))
				})
				// Extract the port's host. This should never be `None`.
				.filter_map(|(proxied_port, _)| {
					proxied_port
						.ingress_hostnames
						.first()
						.map(|hostname| (proxied_port, hostname))
				})
				.map(|(proxied_port, hostname)| {
					GlobalResult::Ok(models::MatchmakerJoinPort {
						host: Some(format!("{}:{}", hostname, proxied_port.ingress_port)),
						hostname: hostname.clone(),
						port: Some(proxied_port.ingress_port.try_into()?),
						port_range: None,
						is_tls: matches!(
							mm_proxy_protocol,
							MmProxyProtocol::Https | MmProxyProtocol::TcpTls
						),
					})
				})
				.next()
				.transpose()?
		}
		(MmProxyKind::None, MmProxyProtocol::Tcp | MmProxyProtocol::Udp) => {
			let port_range = internal_unwrap!(port.port_range);

			let network = internal_unwrap_owned!(
				run.networks.iter().find(|x| x.mode == "host"),
				"missing host network"
			);

			Some(models::MatchmakerJoinPort {
				host: None,
				hostname: network.ip.clone(),
				port: None,
				port_range: Some(Box::new(models::MatchmakerJoinPortRange {
					min: port_range.min.try_into()?,
					max: port_range.max.try_into()?,
				})),
				is_tls: false,
			})
		}
		(
			MmProxyKind::None,
			MmProxyProtocol::Http | MmProxyProtocol::Https | MmProxyProtocol::TcpTls,
		) => {
			internal_panic!("invalid http proxy protocol with host network")
		}
	};

	GlobalResult::Ok(join_info_port.map(|x| (port.label.clone(), x)))
}

fn test_mm_and_job_proxy_protocol_eq(
	mm_proxy_protocol: backend::matchmaker::lobby_runtime::ProxyProtocol,
	job_proxy_protocol: backend::job::ProxyProtocol,
) -> bool {
	use backend::job::ProxyProtocol as JPP;
	use backend::matchmaker::lobby_runtime::ProxyProtocol as MPP;

	match (mm_proxy_protocol, job_proxy_protocol) {
		(MPP::Http, JPP::Http) => true,
		(MPP::Https, JPP::Https) => true,
		(MPP::Tcp, JPP::Tcp) => true,
		(MPP::TcpTls, JPP::TcpTls) => true,
		(MPP::Udp, JPP::Udp) => true,
		_ => false,
	}
}
