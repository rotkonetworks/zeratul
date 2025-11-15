//! Panel management for resizable/movable UI

use ratatui::layout::Rect;
use super::DragType;

#[derive(Clone)]
pub struct Panel {
    pub panel_type: PanelType,
    pub rect: Rect,
    pub layout: PanelLayout,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PanelType {
    OrderBook,
    Chart,
    OrderEntry,
    Positions,
    RecentTrades,
}

#[derive(Clone, Copy)]
pub struct PanelLayout {
    pub border_size: u16,
}

impl Default for PanelLayout {
    fn default() -> Self {
        Self { border_size: 1 }
    }
}

impl Panel {
    pub fn new(panel_type: PanelType, rect: Rect) -> Self {
        Self {
            panel_type,
            rect,
            layout: PanelLayout::default(),
        }
    }

    /// Check if mouse is over a resize handle
    pub fn check_resize_handle(&self, x: u16, y: u16) -> Option<DragType> {
        let handle_size = 2;

        // Check corner (bottom-right)
        if x >= self.rect.x + self.rect.width - handle_size
            && x < self.rect.x + self.rect.width
            && y >= self.rect.y + self.rect.height - handle_size
            && y < self.rect.y + self.rect.height
        {
            return Some(DragType::ResizeCorner);
        }

        // Check right edge
        if x >= self.rect.x + self.rect.width - handle_size
            && x < self.rect.x + self.rect.width
            && y >= self.rect.y
            && y < self.rect.y + self.rect.height
        {
            return Some(DragType::ResizeRight);
        }

        // Check bottom edge
        if x >= self.rect.x
            && x < self.rect.x + self.rect.width
            && y >= self.rect.y + self.rect.height - handle_size
            && y < self.rect.y + self.rect.height
        {
            return Some(DragType::ResizeBottom);
        }

        None
    }

    pub fn title(&self) -> &str {
        match self.panel_type {
            PanelType::OrderBook => "Order Book",
            PanelType::Chart => "Price Chart",
            PanelType::OrderEntry => "Place Order",
            PanelType::Positions => "Positions & Fills",
            PanelType::RecentTrades => "Recent Trades",
        }
    }
}
