mod config;
mod error;
mod extractors;
#[cfg(feature = "processes")]
mod processor;
mod routes;

pub use config::Config;
pub use error::Error;
#[cfg(feature = "processes")]
pub use processor::Processor;

use std::sync::{Arc, RwLock};

use axum::{extract::Extension, routing::get, Router};
use openapiv3::OpenAPI;
use tower::ServiceBuilder;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

use ogcapi_drivers::{
    postgres::Db, CollectionTransactions, EdrQuerier, FeatureTransactions, JobHandler,
    StyleTransactions, TileTransactions,
};
use ogcapi_types::common::{
    link_rel::{CONFORMANCE, SELF, SERVICE_DESC},
    media_type::{JSON, OPEN_API_JSON},
    Conformance, LandingPage, Link,
};

pub type Result<T, E = Error> = std::result::Result<T, E>;

// static OPENAPI: &[u8; 29696] = include_bytes!("../openapi.yaml");
static OPENAPI: &[u8; 122145] = include_bytes!("../openapi-edr.yaml");

// #[derive(Clone)]
pub struct State {
    pub drivers: Drivers,
    pub root: RwLock<LandingPage>,
    pub conformance: RwLock<Conformance>,
    pub openapi: OpenAPI,
}

// TODO: Introduce service trait
pub struct Drivers {
    collections: Box<dyn CollectionTransactions>,
    features: Box<dyn FeatureTransactions>,
    edr: Box<dyn EdrQuerier>,
    jobs: Box<dyn JobHandler>,
    styles: Box<dyn StyleTransactions>,
    tiles: Box<dyn TileTransactions>,
}

pub async fn app(db: Db) -> Router {
    // state
    let openapi: OpenAPI = serde_yaml::from_slice(OPENAPI).unwrap();

    let root = RwLock::new(LandingPage {
        #[cfg(feature = "stac")]
        id: "root".to_string(),
        title: Some(openapi.info.title.to_owned()),
        description: openapi.info.description.to_owned(),
        links: vec![
            Link::new(".", SELF).title("This document").mediatype(JSON),
            Link::new("api", SERVICE_DESC)
                .title("The Open API definition")
                .mediatype(OPEN_API_JSON),
            Link::new("conformance", CONFORMANCE)
                .title("OGC conformance classes implemented by this API")
                .mediatype(JSON),
        ],
        ..Default::default()
    });

    let conformance = RwLock::new(Conformance {
        conforms_to: vec![
            "http://www.opengis.net/spec/ogcapi-common-1/1.0/req/core".to_string(),
            "http://www.opengis.net/spec/ogcapi-common-2/1.0/req/collections".to_string(),
            "http://www.opengis.net/spec/ogcapi_common-2/1.0/req/json".to_string(),
        ],
    });

    let drivers = Drivers {
        collections: Box::new(db.clone()),
        features: Box::new(db.clone()),
        edr: Box::new(db.clone()),
        jobs: Box::new(db.clone()),
        styles: Box::new(db.clone()),
        tiles: Box::new(db),
    };

    let state = State {
        drivers,
        root,
        conformance,
        openapi,
    };

    // routes
    let router = Router::new()
        .route("/", get(routes::root))
        .route("/api", get(routes::api))
        .route("/redoc", get(routes::redoc))
        .route("/swagger", get(routes::swagger))
        .route("/conformance", get(routes::conformance));

    let router = router.merge(routes::collections::router(&state));

    #[cfg(feature = "features")]
    let router = router.merge(routes::features::router(&state));

    #[cfg(feature = "edr")]
    let router = router.merge(routes::edr::router(&state));

    #[cfg(feature = "styles")]
    let router = router.merge(routes::styles::router(&state));

    #[cfg(feature = "tiles")]
    let router = router.merge(routes::tiles::router(&state));

    #[cfg(feature = "processes")]
    let router = router.merge(routes::processes::router(
        &state,
        vec![
            Box::new(processor::Greeter),
            Box::new(processor::AssetLoader),
        ],
    ));

    // middleware stack
    router.layer(
        ServiceBuilder::new()
            .layer(TraceLayer::new_for_http())
            .layer(CompressionLayer::new())
            .layer(CorsLayer::permissive())
            .layer(Extension(Arc::new(state))),
    )
}
