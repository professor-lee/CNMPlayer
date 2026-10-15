use super::*;

pub(crate) type CoverFuture = SharedTask<Arc<DynamicImage>>;

pub(crate) type AsciiFuture = SharedTask<String>;

#[derive(Clone, Default)]
pub struct CoverFetchState {
    pub url: Option<String>,
    pub image: Option<CoverFuture>,
    pub(crate) ascii: Option<AsciiFuture>,
    pub(crate) size: Size,
    pub(crate) identity: u64,
    pub(crate) wake: WakeSignal,
}

impl CoverFetchState {
    pub fn load(&mut self, api: ApiState, url: String) {
        self.wake = api.wake_signal();
        self.identity = next_list_generation();
        let cover_url = url.clone();
        let fut = Box::pin(async move { api.fetch_cover_image(&cover_url).await.ok() });
        self.image = Some(spawn_shared(fut, self.wake.clone()));
        self.url = Some(url);
        self.size = Size::ZERO;
        self.ascii = None;
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        covers: &mut CoverPresentation,
        area: Rect,
        text_style: Style,
        bg_style: Option<Style>,
        draw_ascii: bool,
    ) {
        self.render_rows(
            frame,
            covers,
            area,
            area.height,
            0..area.height,
            text_style,
            bg_style,
            draw_ascii,
        );
    }

    /// Partial views copy rows of the full-size prepared surface. Scrolling does
    /// not create additional image geometry, codecs, or cropped cache entries.
    #[allow(clippy::too_many_arguments)]
    pub fn render_rows(
        &mut self,
        frame: &mut Frame,
        covers: &mut CoverPresentation,
        area: Rect,
        full_rows: u16,
        visible: Range<u16>,
        text_style: Style,
        bg_style: Option<Style>,
        draw_ascii: bool,
    ) {
        let visible_rows = visible
            .end
            .min(full_rows)
            .saturating_sub(visible.start.min(full_rows));
        if area.is_empty() || full_rows == 0 || visible_rows == 0 {
            return;
        }
        let area = Rect {
            height: area.height.min(visible_rows),
            ..area
        };
        if area.is_empty() {
            return;
        }

        if let Some(bg) = bg_style {
            frame.render_widget(Block::default().style(bg), area);
        }

        // 缓存按**整块图**尺寸键控：部分可见时不会每帧重建。
        let size = Size::new(area.width, full_rows);
        if draw_ascii {
            if (self.ascii.is_none() || self.size != size)
                && let Some(bytes) = peek_shared_future(&self.image)
            {
                self.ascii = Some(make_ascii_future(
                    bytes.clone(),
                    area.width,
                    full_rows,
                    self.wake.clone(),
                ));
                self.size = size;
            }
            let ascii = match peek_shared_future(&self.ascii) {
                Some(x) => x.clone(),
                None => placeholder_cover_ascii(area.width, full_rows, '░'),
            };
            frame.render_widget(
                Paragraph::new(ascii)
                    .style(text_style)
                    .scroll((visible.start, 0)),
                area,
            );
            return;
        }

        if let Some(image) = peek_shared_future(&self.image) {
            covers.show(
                frame,
                CoverKey {
                    hash: self.identity,
                    width: area.width,
                    height: full_rows,
                },
                image,
                area,
                visible.start,
            );
        }
    }
}

pub(crate) fn make_ascii_future(
    image: Arc<DynamicImage>,
    width: u16,
    height: u16,
    wake: WakeSignal,
) -> AsciiFuture {
    let fut = Box::pin(async move {
        compio::runtime::spawn_blocking(move || render_cover_ascii(image, width, height))
            .await
            .ok()
            .flatten()
    });
    spawn_shared(fut, wake)
}

pub(crate) fn placeholder_cover_ascii(width: u16, height: u16, ch: char) -> String {
    if width == 0 || height == 0 {
        return String::new();
    }

    let row = ch.to_string().repeat(width as usize);
    let mut out = String::new();
    for _ in 0..height {
        out.push_str(&row);
        out.push('\n');
    }
    out
}
