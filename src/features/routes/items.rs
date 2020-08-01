use crate::common::{ContentType, Link, LinkRelation, CRS, Datetime};
use crate::features::schema::{Feature, FeatureCollection};
use crate::Features;
use chrono::{SecondsFormat, Utc};
use serde::Deserialize;
use sqlx::types::Json;
use sqlx::Done;
use tide::http::Method;
use tide::{Body, Request, Response, Result};

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct Query {
    limit: Option<isize>,
    offset: Option<isize>,
    bbox: Option<Vec<f64>>,
    bbox_crs: Option<CRS>,
    datetime: Option<Datetime>,
    crs: Option<CRS>,
}

impl Query {
    fn to_string(&self) -> String {
        let mut query_str = vec![];
        if let Some(limit) = self.limit {
            query_str.push(format!("limit={}", limit));
        }
        if let Some(offset) = self.offset {
            query_str.push(format!("offset={}", offset));
        }
        if let Some(bbox) = &self.bbox {
            query_str.push(format!(
                "bbox={}",
                bbox.iter()
                    .map(|coord| coord.to_string())
                    .collect::<Vec<String>>()
                    .join(",")
            ));
        }
        if let Some(bbox_crs) = &self.bbox_crs {
            query_str.push(format!("bboxCrs={}", bbox_crs.to_string()));
        }
        if let Some(datetime) = &self.datetime {
            query_str.push(format!("datetime={}", datetime.to_string()));
        }
        if let Some(crs) = &self.crs {
            query_str.push(format!("crs={}", crs.to_string()));
        }
        query_str.join("&")
    }

    fn to_string_with_offset(&self, offset: isize) -> String {
        let mut new_query = self.clone();
        new_query.offset = Some(offset);
        new_query.to_string()
    }

    pub fn make_envelope(&self) -> Option<String> {
        if let Some(mut bbox) = self.bbox.to_owned() {
            let srid = self
                .bbox_crs
                .clone()
                .unwrap_or_else(|| CRS::default())
                .code
                .clone()
                .parse::<i32>()
                .expect("Parse bbox crs EPSG code");

            // downgrade 3d bbox to 2d
            if bbox.len() == 6 {
                bbox.remove(5);
                bbox.remove(2);
            }

            if bbox.len() == 4 {
                Some(format!(
                    "ST_MakeEnvelope ( {xmin}, {ymin}, {xmax}, {ymax}, {my_srid} )",
                    xmin = bbox[0],
                    ymin = bbox[1],
                    xmax = bbox[2],
                    ymax = bbox[3],
                    my_srid = srid
                ))
            } else {
                None
            }
        } else {
            None
        }
    }
}

pub async fn handle_item(mut req: Request<Features>) -> tide::Result {
    let url = req.url().clone();
    let method = req.method();

    let id: Option<String> = if method != Method::Post {
        Some(req.param("id")?)
    } else {
        None
    };

    let collection: String = req.param("collection")?;

    let mut res = Response::new(200);
    let mut feature: Feature;

    match method {
        Method::Get => {
            let sql = r#"
            SELECT id, type, ST_AsGeoJSON(geometry)::jsonb as geometry, properties, links
            FROM data.features
            WHERE collection = $1 AND id = $2
            "#;
            feature = sqlx::query_as(sql)
                .bind(collection)
                .bind(&id)
                .fetch_one(&req.state().pool)
                .await?;
        }
        Method::Post | Method::Put => {
            feature = req.body_json().await?;

            let mut sql = if method == Method::Post {
                vec![
                    "INSERT INTO data.features",
                    "(id, type, properties, geometry, links)",
                    "VALUES ($1, $2, $3, ST_GeomFromGeoJSON($4), $5)",
                ]
            } else {
                vec![
                    "UPDATE data.features",
                    "SET type = $2, properties = $3, geometry = ST_GeomFromGeoJSON($4), links = $5)",
                    "WHERE id = $1",
                ]
            };
            sql.push(
                "RETURNING id, type, properties, ST_AsGeoJSON(geometry)::jsonb as geometry, links",
            );

            let mut tx = req.state().pool.begin().await?;
            feature = sqlx::query_as(&sql.join(" ").as_str())
                .bind(&feature.id)
                .bind(&feature.r#type)
                .bind(&feature.properties)
                .bind(&feature.geometry)
                .bind(&feature.links)
                .fetch_one(&mut tx)
                .await?;
            tx.commit().await?;
        }
        Method::Delete => {
            let mut tx = req.state().pool.begin().await?;

            let _deleted = sqlx::query("DELETE FROM data.features WHERE id = $1")
                .bind(id)
                .execute(&mut tx)
                .await?;

            tx.commit().await?;

            return Ok(res);
        }
        _ => unimplemented!(),
    }

    feature.links = Some(Json(vec![
        Link {
            href: url.to_string(),
            r#type: Some(ContentType::GEOJSON),
            ..Default::default()
        },
        Link {
            href: url.as_str().replace(&format!("/items/{}", id.unwrap()), ""),
            rel: LinkRelation::Collection,
            r#type: Some(ContentType::GEOJSON),
            ..Default::default()
        },
    ]));

    res.set_content_type(ContentType::GEOJSON);
    res.set_body(Body::from_json(&feature)?);
    Ok(res)
}

pub async fn handle_items(req: Request<Features>) -> Result {
    let mut url = req.url().to_owned();

    let collection: String = req.param("collection")?;

    let mut query: Query = req.query()?;

    let srid = match &query.crs {
        Some(crs) => crs.code.parse::<i32>().unwrap_or(4326),
        None => 4326,
    };

    let mut sql = vec![
        format!("SELECT id, type, ST_AsGeoJSON( ST_Transform (geometry, {}))::jsonb as geometry, properties, links
        FROM data.features
        WHERE collection = $1", srid)
    ];

    if query.bbox.is_some() {
        if let Some(envelop) = query.make_envelope() {
            sql.push(format!("WHERE geometry && {}", envelop));
        }
    }

    let number_matched = sqlx::query(sql.join(" ").as_str())
        .bind(&collection)
        .execute(&req.state().pool)
        .await?
        .rows_affected();

    let mut links = vec![Link {
        href: url.to_string(),
        r#type: Some(ContentType::GEOJSON),
        ..Default::default()
    }];

    // pagination
    if let Some(limit) = query.limit {
        sql.push("ORDER BY id".to_string());
        sql.push(format!("LIMIT {}", limit));

        if query.offset.is_none() {
            query.offset = Some(0);
        }

        if let Some(offset) = query.offset {
            sql.push(format!("OFFSET {}", offset));

            if offset != 0 && offset >= limit {
                url.set_query(Some(&query.to_string_with_offset(offset - limit)));
                let previous = Link {
                    href: url.to_string(),
                    rel: LinkRelation::Previous,
                    r#type: Some(ContentType::GEOJSON),
                    ..Default::default()
                };
                links.push(previous);
            }

            if !(offset + limit) as u64 >= number_matched {
                url.set_query(Some(&query.to_string_with_offset(offset + limit)));
                let next = Link {
                    href: url.to_string(),
                    rel: LinkRelation::Next,
                    r#type: Some(ContentType::GEOJSON),
                    ..Default::default()
                };
                links.push(next);
            }
        }
    }

    let features: Vec<Feature> = sqlx::query_as(sql.join(" ").as_str())
        .bind(&collection)
        .fetch_all(&req.state().pool)
        .await?;

    let number_returned = features.len();

    let feature_collection = FeatureCollection {
        r#type: "FeatureCollection".to_string(),
        features,
        links: Some(links),
        time_stamp: Some(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)),
        number_matched: Some(number_matched),
        number_returned: Some(number_returned),
    };

    let mut res = Response::new(200);
    res.set_content_type(ContentType::GEOJSON);
    res.set_body(Body::from_json(&feature_collection)?);
    Ok(res)
}