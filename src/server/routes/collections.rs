// use serde::Deserialize;
// use serde_with::{serde_as, DisplayFromStr};
use sqlx::types::Json;
use tide::http::url::Position;
use tide::{Body, Request, Response, Result, Server};

use crate::common::core::LinkRel;
use crate::common::{
    collections::{Collection, Collections},
    core::{Link, MediaType},
    crs::Crs,
};
use crate::server::State;

const CONFORMANCE: [&str; 3] = [
    "http://www.opengis.net/spec/ogcapi-common-1/1.0/req/core",
    "http://www.opengis.net/spec/ogcapi-common-2/1.0/req/collections",
    "http://www.opengis.net/spec/ogcapi_common-2/1.0/req/json",
];

// #[serde_as]
// #[derive(Deserialize, Debug, Clone)]
// #[serde(deny_unknown_fields)]
// struct Query {
//     bbox: Option<Bbox>,
//     #[serde_as(as = "Option<DisplayFromStr>")]
//     bbox_crs: Option<Crs>,
//     #[serde_as(as = "Option<DisplayFromStr>")]
//     datetime: Option<Datetime>,
//     limit: Option<isize>,
//     offset: Option<isize>,
// }

// impl Query {
//     fn to_string(&self) -> String {
//         let mut query_str = vec![];
//         if let Some(limit) = self.limit {
//             query_str.push(format!("limit={}", limit));
//         }
//         if let Some(offset) = self.offset {
//             query_str.push(format!("offset={}", offset));
//         }
//         if let Some(bbox) = &self.bbox {
//             query_str.push(format!("bbox={}", bbox));
//         }
//         if let Some(bbox_crs) = &self.bbox_crs {
//             query_str.push(format!("bboxCrs={}", bbox_crs.to_string()));
//         }
//         if let Some(datetime) = &self.datetime {
//             query_str.push(format!("datetime={}", datetime.to_string()));
//         }
//         query_str.join("&")
//     }
// }

async fn collections(req: Request<State>) -> Result {
    let url = req.url();

    //let mut query: Query = req.query()?;

    let mut collections: Vec<Json<Collection>> =
        sqlx::query_scalar("SELECT collection FROM meta.collections")
            .fetch_all(&req.state().db.pool)
            .await?;

    let collections = collections
        .iter_mut()
        .map(|c| {
            let base = &url[..Position::AfterPath];
            c.0.links.append(&mut vec![
                Link::new(&format!("{}/{}", base, c.id)),
                Link::new(&format!("{}/{}/items", base, c.id))
                    .mime(MediaType::GeoJSON)
                    .title(format!("Items of {}", c.title.as_ref().unwrap_or(&c.id))),
            ]);
            c.0.to_owned()
        })
        .collect();

    let collections = Collections {
        links: vec![Link::new(url.as_str())
            .mime(MediaType::JSON)
            .title("this document".to_string())],
        crs: Some(vec![Crs::default(), Crs::from(4326)]),
        collections,
        ..Default::default()
    };

    let mut res = Response::new(200);
    res.set_body(Body::from_json(&collections)?);
    Ok(res)
}

/// Create new collection metadata
async fn insert(mut req: Request<State>) -> Result {
    let collection: Collection = req.body_json().await?;

    let location = req.state().db.insert_collection(&collection).await?;

    let mut res = Response::new(201);
    res.insert_header("Location", location);
    Ok(res)
}

/// Get collection metadata
async fn get(req: Request<State>) -> Result {
    let id = req.param("collectionId")?;

    let mut collection = req.state().db.select_collection(id).await?;

    collection.links.push(
        Link::new(&format!("{}/items", &req.url()[..Position::AfterPath]))
            .mime(MediaType::GeoJSON)
            .title(format!(
                "Items of {}",
                collection.title.as_ref().unwrap_or(&collection.id)
            )),
    );

    let mut res = Response::new(200);
    res.set_body(Body::from_json(&collection)?);
    Ok(res)
}

/// Update collection metadata
async fn update(mut req: Request<State>) -> Result {
    let mut collection: Collection = req.body_json().await?;

    let id = req.param("collectionId")?;

    collection.id = id.to_owned();

    req.state().db.update_collection(&collection).await?;

    Ok(Response::new(204))
}

/// Delete collection metadata
async fn delete(req: Request<State>) -> Result {
    let id = req.param("collectionId")?;

    req.state().db.delete_collection(id).await?;

    Ok(Response::new(204))
}

pub(crate) async fn register(app: &mut Server<State>) {
    app.state().root.write().await.links.push(
        Link::new("http://ogcapi.rs/collections")
            .title("Metadata about the resource collections".to_string())
            .relation(LinkRel::Data)
            .mime(MediaType::JSON),
    );

    app.state()
        .conformance
        .write()
        .await
        .conforms_to
        .append(&mut CONFORMANCE.map(String::from).to_vec());

    app.at("/collections").get(collections).post(insert);
    app.at("/collections/:collectionId")
        .get(get)
        .put(update)
        .delete(delete);
}
