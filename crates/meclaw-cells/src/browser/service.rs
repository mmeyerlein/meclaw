//! The door of a `browser` cell: a `page:` topic on a display's socket.
//!
//! There is no socket of this cell's own and no port. The `web` cell's loop
//! answers a `page:<page>` join by asking the process's mount table for a link
//! (ADR-0031, GH #643's second kind), and what it finds under `params.mount` is
//! this opener. So the admission is this cell's own, the frames are this cell's
//! own, and the display learns nothing about either.

use crate::browser::io::LinkCommand;
use crate::browser::pages::Viewport;
use meclaw_colony::surfaces::BoxFuture;
use meclaw_colony::{Link, LinkOpener, LinkRefused, LinkRequest};
use meclaw_core::serde_json::Value;
use tokio::sync::{mpsc, oneshot};

/// Answers a `page:` join by asking the I/O half for a link.
pub struct BrowserLinkOpener {
    /// Where a join goes. The I/O half owns the register and mints the ids.
    pub links: mpsc::Sender<LinkCommand>,
    /// The A-timeout on the answer (`params.external_timeout_ms`).
    ///
    /// The join runs inside the display socket's own loop, so an I/O half that
    /// is busy elsewhere used to hold THAT socket — the one carrying a
    /// person's screen — for as long as it stayed busy. A refusal is a page
    /// with `data-state="error"`; a wait is a screen that stops.
    pub external_timeout: std::time::Duration,
}

impl LinkOpener for BrowserLinkOpener {
    fn open(&self, req: LinkRequest) -> BoxFuture<'static, Result<Link, LinkRefused>> {
        let links = self.links.clone();
        let cap = self.external_timeout;
        Box::pin(async move {
            // The page is the topic suffix and nothing else names it, so a
            // display cannot join one topic and watch another.
            let page = req.session.clone().unwrap_or_default();
            let (answer, wait) = oneshot::channel();
            let deadline = tokio::time::Instant::now() + cap;
            let sent = tokio::time::timeout_at(
                deadline,
                links.send(LinkCommand::Join {
                    page: page.clone(),
                    params: req.params.clone(),
                    answer,
                }),
            )
            .await;
            match sent {
                Err(_) => {
                    return Err(LinkRefused {
                        status: 504,
                        detail: "this browser did not answer the join in time".to_string(),
                    });
                }
                Ok(Err(_)) => {
                    return Err(LinkRefused {
                        status: 503,
                        detail: "this browser is not serving".to_string(),
                    });
                }
                Ok(Ok(())) => {}
            }
            match tokio::time::timeout_at(deadline, wait).await {
                Err(_) => Err(LinkRefused {
                    status: 504,
                    detail: "this browser did not answer the join in time".to_string(),
                }),
                Ok(Ok(out)) => out,
                Ok(Err(_)) => Err(LinkRefused {
                    status: 503,
                    detail: "this browser is not serving".to_string(),
                }),
            }
        })
    }
}

/// The viewport a join brought, ONE level deep (OR-G32).
///
/// The join payload is flat and the socket door hands on everything except the
/// `mount` that chose it, so a viewport arrives at `params.viewport` and never
/// at `params.params.viewport`. A cell that read both would make the difference
/// unobservable, which is how a shape like this stops being a contract.
pub fn viewport_of_join(params: &Value) -> Option<Viewport> {
    let v = params.get("viewport")?.as_object()?;
    let dim = |key: &str| -> Option<u32> {
        v.get(key)
            .and_then(Value::as_u64)
            .filter(|n| *n > 0)
            .and_then(|n| u32::try_from(n).ok())
    };
    Some(Viewport {
        width: dim("width")?,
        height: dim("height")?,
        dpr: v
            .get("dpr")
            .and_then(Value::as_f64)
            .filter(|d| *d > 0.0)
            .unwrap_or(1.0),
        mobile: v.get("mobile").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// The 16-byte head every picture frame carries.
///
/// Four big-endian `u32`s — width, height, scroll x, scroll y — in CSS pixels
/// of the page's viewport. A client sizes its canvas from the first two and
/// places what it draws with the other two, which is why they travel with every
/// frame rather than with the join: a page that scrolls does not rejoin.
pub fn head(width: u32, height: u32, scroll_x: u32, scroll_y: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&width.to_be_bytes());
    out[4..8].copy_from_slice(&height.to_be_bytes());
    out[8..12].copy_from_slice(&scroll_x.to_be_bytes());
    out[12..16].copy_from_slice(&scroll_y.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    /// One join, refused rather than held, when the I/O half is busy.
    ///
    /// The join runs inside the DISPLAY socket's loop. An unbounded wait here
    /// is not a slow page: it is a person's whole screen stopping, and behind
    /// it there is nothing — a long-running cell has `cell.timeout: -1` and no
    /// message-timeout backstop.
    #[tokio::test]
    async fn a_busy_browser_refuses_a_join_instead_of_holding_the_socket() {
        let (links, _keep) = mpsc::channel::<LinkCommand>(1);
        // The one slot, taken by a join nobody is serving.
        let (answer, _) = oneshot::channel();
        links
            .send(LinkCommand::Join {
                page: "card-0".to_string(),
                params: Value::Null,
                answer,
            })
            .await
            .expect("the one slot");
        let opener = BrowserLinkOpener {
            links,
            external_timeout: std::time::Duration::from_millis(200),
        };
        let started = std::time::Instant::now();
        let refused: Result<Link, LinkRefused> = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            opener.open(LinkRequest {
                session: Some("card-1".to_string()),
                params: Value::Null,
                ..Default::default()
            }),
        )
        .await
        .expect("the join ends on its own, which is the whole point");
        let refused = match refused {
            Err(refused) => refused,
            // `Link` carries two channels and no `Debug`, so the arm is
            // written out rather than reached through `expect_err`.
            Ok(_) => panic!("a busy browser handed out a link"),
        };
        assert_eq!(refused.status, 504);
        assert!(
            refused.detail.contains("in time"),
            "the page shows `data-state=\"error\"` and says why: {}",
            refused.detail
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "and it ends at the A-timeout: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_viewport_is_read_one_level_deep_and_not_two() {
        let flat = json!({"viewport": {"width": 960, "height": 600, "dpr": 2.0, "mobile": true}});
        let v = viewport_of_join(&flat).expect("one level deep");
        assert_eq!((v.width, v.height, v.mobile), (960, 600, true));
        assert!(
            viewport_of_join(&json!({"params": {"viewport": {"width": 960, "height": 600}}}))
                .is_none(),
            "a nested one brings nothing: the door already unwrapped it (OR-G32)"
        );
        assert!(viewport_of_join(&json!({})).is_none());
        assert!(
            viewport_of_join(&json!({"viewport": {"width": 0, "height": 600}})).is_none(),
            "a viewport with no width is not a shape"
        );
    }

    #[test]
    fn the_head_is_four_big_endian_numbers() {
        let h = head(960, 600, 0, 120);
        assert_eq!(h.len(), 16);
        assert_eq!(&h[0..4], &960u32.to_be_bytes());
        assert_eq!(&h[4..8], &600u32.to_be_bytes());
        assert_eq!(&h[8..12], &0u32.to_be_bytes());
        assert_eq!(&h[12..16], &120u32.to_be_bytes());
    }
}
