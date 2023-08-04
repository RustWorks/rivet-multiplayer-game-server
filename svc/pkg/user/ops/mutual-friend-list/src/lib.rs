use proto::backend::pkg::*;
use rivet_operation::prelude::*;

#[derive(Debug, sqlx::FromRow)]
struct Follow {
	follower_user_id: Uuid,
	following_user_id: Uuid,
	create_ts: i64,
}

#[operation(name = "user-mutual-friend-list")]
async fn handle(
	ctx: OperationContext<user::mutual_friend_list::Request>,
) -> GlobalResult<user::mutual_friend_list::Response> {
	let user_a_id = internal_unwrap!(ctx.user_a_id).as_uuid();
	let user_b_id = internal_unwrap!(ctx.user_b_id).as_uuid();

	let limit = ctx.limit;

	internal_assert!(limit != 0, "limit too low");
	internal_assert!(limit <= 32, "limit too high");

	let mutual_friends = sqlx::query_as::<_, Follow>(indoc!(
		"
		-- Mutual check A-C
		SELECT aa.follower_user_id, aa.following_user_id, aa.create_ts
		FROM (
			SELECT follower_user_id, following_user_id, create_ts
			FROM (
				SELECT
					uf.follower_user_id, uf.following_user_id, create_ts,
					EXISTS (
						SELECT 1
						FROM user_follows AS uf2
						WHERE
							uf2.follower_user_id = uf.following_user_id AND
							uf2.following_user_id = uf.follower_user_id
					) AS is_mutual_ac
				FROM user_follows AS uf
				WHERE uf.following_user_id = $1
			) AS q
			WHERE is_mutual_ac AND create_ts > $3
		) AS aa
		-- Mutual check B-C
		INNER JOIN (
			SELECT follower_user_id, following_user_id
			FROM (
				SELECT
					uf.follower_user_id, uf.following_user_id, create_ts,
					EXISTS (
						SELECT 1
						FROM user_follows AS uf2
						WHERE
							uf2.follower_user_id = uf.following_user_id AND
							uf2.following_user_id = uf.follower_user_id
					) AS is_mutual_bc
				FROM user_follows AS uf
				WHERE uf.following_user_id = $2
			) AS q
			WHERE is_mutual_bc AND create_ts > $3
		) AS bb
		ON aa.follower_user_id = bb.follower_user_id
		-- Mutual check A-B
		INNER JOIN (
			SELECT follower_user_id, following_user_id
			FROM (
				SELECT
					uf.follower_user_id, uf.following_user_id, create_ts,
					EXISTS (
						SELECT 1
						FROM user_follows AS uf2
						WHERE
							uf2.follower_user_id = uf.following_user_id AND
							uf2.following_user_id = uf.follower_user_id
					) AS is_mutual_ab
				FROM user_follows AS uf
				WHERE uf.follower_user_id = $1 AND uf.following_user_id = $2
			) AS q
			WHERE is_mutual_ab AND create_ts > $3
		) AS cc
		ON bb.following_user_id = cc.follower_user_id OR bb.following_user_id = cc.following_user_id
		ORDER BY create_ts DESC
		LIMIT $4
		"
	))
	.bind(user_a_id)
	.bind(user_b_id)
	.bind(ctx.anchor.unwrap_or_default())
	.bind(limit as i64)
	.fetch_all(&ctx.crdb("db-user-follow").await?)
	.await?;

	let anchor = mutual_friends
		.last()
		.and_then(|follow| (mutual_friends.len() >= limit as usize).then_some(follow.create_ts));

	Ok(user::mutual_friend_list::Response {
		mutual_friends: mutual_friends
			.into_iter()
			.map(
				|mutual_friend| user::mutual_friend_list::response::MutualFriend {
					user_id: if mutual_friend.follower_user_id == user_a_id {
						Some(mutual_friend.following_user_id.into())
					} else {
						Some(mutual_friend.follower_user_id.into())
					},
					create_ts: mutual_friend.create_ts,
				},
			)
			.collect::<Vec<_>>(),
		anchor,
	})
}
