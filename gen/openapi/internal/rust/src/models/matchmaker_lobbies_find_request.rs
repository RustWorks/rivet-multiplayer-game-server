/*
 * Rivet API
 *
 * No description provided (generated by Openapi Generator https://github.com/openapitools/openapi-generator)
 *
 * The version of the OpenAPI document: 0.0.1
 * 
 * Generated by: https://openapi-generator.tech
 */




#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct MatchmakerLobbiesFindRequest {
    #[serde(rename = "captcha", skip_serializing_if = "Option::is_none")]
    pub captcha: Option<Box<crate::models::CaptchaConfig>>,
    #[serde(rename = "game_modes")]
    pub game_modes: Vec<String>,
    #[serde(rename = "prevent_auto_create_lobby", skip_serializing_if = "Option::is_none")]
    pub prevent_auto_create_lobby: Option<bool>,
    #[serde(rename = "regions", skip_serializing_if = "Option::is_none")]
    pub regions: Option<Vec<String>>,
    #[serde(rename = "verification_data", default, with = "::serde_with::rust::double_option", skip_serializing_if = "Option::is_none")]
    pub verification_data: Option<Option<serde_json::Value>>,
}

impl MatchmakerLobbiesFindRequest {
    pub fn new(game_modes: Vec<String>) -> MatchmakerLobbiesFindRequest {
        MatchmakerLobbiesFindRequest {
            captcha: None,
            game_modes,
            prevent_auto_create_lobby: None,
            regions: None,
            verification_data: None,
        }
    }
}


