/// Mode for the sidebar display
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarMode {
    #[default]
    Comments,
    Actions,
    PendingReview,
}

impl SidebarMode {
    pub fn toggle(&self) -> Self {
        match self {
            SidebarMode::Comments => SidebarMode::Actions,
            SidebarMode::Actions => SidebarMode::Comments,
            SidebarMode::PendingReview => SidebarMode::PendingReview,
        }
    }

    pub fn to_display(&self) -> &'static str {
        match self {
            SidebarMode::Comments => "Comments",
            SidebarMode::Actions => "Actions",
            SidebarMode::PendingReview => "Pending Review",
        }
    }
}
