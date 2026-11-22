use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use garmin_device::{TransportKind, parse_manifest};
use garmin_fixtures::firmware::{
    EDGE_32_20, FENIX_24_20, FENIX_GSP_22_44, FONT_METADATA, GSP_METADATA, VIDEO_METADATA,
};
use garmin_model::map::MapVersion;
use garmin_update::firmware::UpdateMetadata;
use serde::Deserialize;
use std::collections::HashMap;

/// Small, inert payloads let the loopback host exercise real transfer and integrity checks.
pub(super) fn files() -> HashMap<String, Bytes> {
    let mut files = HashMap::new();
    for part in [
        "006-D1074-00",
        "006-V4440-00",
        "006-DB477-00",
        "006-DA674-00",
        "006-V4532-00",
        "006-DB775-00",
        "006-D9971-49",
    ] {
        files.insert(part.to_owned(), Bytes::from_static(part.as_bytes()));
    }
    for fixture in [GSP_METADATA, VIDEO_METADATA, FONT_METADATA] {
        let record: UpdateMetadata = serde_json::from_str(fixture).expect("valid metadata fixture");
        files.insert(
            format!("{}-META", record.part_number),
            Bytes::from(serde_json::to_vec(&record).expect("serializable metadata")),
        );
    }
    files
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Request {
    garmin_device_xml: String,
    is_user_interactive: bool,
    client_info: ClientInfo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientInfo {
    client_type: String,
    operating_system_type: String,
    operating_system_version: String,
    locale_code: String,
}

pub(super) async fn updates(
    State(state): State<super::AppState>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> Result<impl IntoResponse, StatusCode> {
    super::validate_client_headers(&headers)?;
    let info = request.client_info;
    if request.is_user_interactive
        || info.client_type != "express"
        || info.operating_system_type != "Windows"
        || info.operating_system_version != "10.0"
        || info.locale_code.is_empty()
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let device = parse_manifest(
        &request.garmin_device_xml,
        TransportKind::MassStorage,
        "loopback".into(),
    )
    .map_err(|_| StatusCode::BAD_REQUEST)?;
    let inventory = device.capabilities();
    let body = match inventory.model().part_number() {
        Some("006-B4440-00") => EDGE_32_20.catalog,
        Some("006-B4532-00" | "006-FAKE-01")
            if inventory.installed_update_files().iter().any(|file| {
                file.part_number == "006-V4532-00" && file.version < MapVersion::new(24, 20)
            }) =>
        {
            FENIX_GSP_22_44.catalog
        }
        Some("006-B4532-00" | "006-FAKE-01") => FENIX_24_20.catalog,
        _ => {
            return Ok(
                Json(serde_json::json!({"updates": [], "accessLevel": "live"})).into_response(),
            );
        }
    };
    let mut catalog: serde_json::Value =
        serde_json::from_str(body).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    for offer in catalog["updates"]
        .as_array_mut()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
    {
        let part = offer["partNumber"]
            .as_str()
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        let file = state
            .files
            .get(part)
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        let url = state
            .base
            .join(&format!("downloads/{part}"))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let delivery = &mut offer["deliverable"];
        delivery["secureDownloadUrl"] = url.as_str().into();
        delivery["downloadUrl"] = url.as_str().into();
        delivery["md5"] = super::md5_hex(file).into();
        delivery["sizeInBytes"] = file.len().into();
    }
    Ok(Json(catalog).into_response())
}
