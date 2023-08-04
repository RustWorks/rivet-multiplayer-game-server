use proto::backend;
use rivet_chat_server::models;
use rivet_convert::ApiInto;
use rivet_operation::prelude::*;

use crate::convert;

pub fn message(
	current_user_id: Uuid,
	message: &backend::chat::Message,
	users: &[backend::user::User],
	games: &[convert::GameWithNamespaceIds],
) -> GlobalResult<models::ChatMessage> {
	// Read body message
	let backend_body_kind = internal_unwrap!(message.body);
	let backend_body_kind = internal_unwrap!(backend_body_kind.kind);

	// Build message body
	let msg_body = {
		use backend::chat::message_body as backend_body;

		match backend_body_kind {
			backend_body::Kind::Custom(backend_body::Custom {
				sender_user_id: _,
				plugin_id: _,
				body: _,
			}) => {
				internal_panic!("Unimplemented");
			}
			backend_body::Kind::Text(backend_body::Text {
				sender_user_id,
				body,
			}) => {
				let sender = internal_unwrap_owned!(users
					.iter()
					.find(|user| &user.user_id == sender_user_id));

				models::ChatMessageBody::Text(models::ChatMessageBodyText {
					sender: convert::identity::handle_without_presence(current_user_id, sender)?,
					body: body.to_owned(),
				})
			}
			backend_body::Kind::ChatCreate(backend_body::ChatCreate {}) => {
				models::ChatMessageBody::ChatCreate(models::ChatMessageBodyChatCreate {})
			}
			backend_body::Kind::Deleted(backend_body::Deleted { sender_user_id }) => {
				let sender = internal_unwrap_owned!(users
					.iter()
					.find(|user| &user.user_id == sender_user_id));

				models::ChatMessageBody::Deleted(models::ChatMessageBodyDeleted {
					sender: convert::identity::handle_without_presence(current_user_id, sender)?,
				})
			}
			backend_body::Kind::UserFollow(backend_body::UserFollow {}) => {
				models::ChatMessageBody::IdentityFollow(models::ChatMessageBodyIdentityFollow {})
			}
			backend_body::Kind::TeamJoin(backend_body::TeamJoin { user_id }) => {
				let user =
					internal_unwrap_owned!(users.iter().find(|user| &user.user_id == user_id));

				models::ChatMessageBody::GroupJoin(models::ChatMessageBodyGroupJoin {
					identity: convert::identity::handle_without_presence(current_user_id, user)?,
				})
			}
			backend_body::Kind::TeamLeave(backend_body::TeamLeave { user_id }) => {
				let user =
					internal_unwrap_owned!(users.iter().find(|user| &user.user_id == user_id));

				models::ChatMessageBody::GroupLeave(models::ChatMessageBodyGroupLeave {
					identity: convert::identity::handle_without_presence(current_user_id, user)?,
				})
			}
			backend_body::Kind::TeamMemberKick(backend_body::TeamMemberKick { user_id }) => {
				let user =
					internal_unwrap_owned!(users.iter().find(|user| &user.user_id == user_id));

				models::ChatMessageBody::GroupMemberKick(models::ChatMessageBodyGroupMemberKick {
					identity: convert::identity::handle_without_presence(current_user_id, user)?,
				})
			}
		}
	};

	Ok(models::ChatMessage {
		chat_message_id: internal_unwrap!(message.chat_message_id)
			.as_uuid()
			.to_string(),
		thread_id: internal_unwrap!(message.thread_id).as_uuid().to_string(),
		send_ts: util::timestamp::to_chrono(message.send_ts)?,
		body: msg_body,
	})
}
