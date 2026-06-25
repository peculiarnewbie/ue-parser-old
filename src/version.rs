//! Central version and package capability context.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PackageFlags(u32);

impl PackageFlags {
    pub const COOKED: u32 = 0x0000_0200;
    pub const UNVERSIONED_PROPERTIES: u32 = 0x0000_2000;
    pub const FILTER_EDITOR_ONLY: u32 = 0x8000_0000;

    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn contains(self, flag: u32) -> bool {
        self.0 & flag == flag
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VersionContext {
    pub legacy_file_version: i32,
    pub legacy_ue3: Option<i32>,
    pub ue4: i32,
    pub ue5: i32,
    pub licensee: i32,
    pub package_flags: PackageFlags,
}

impl VersionContext {
    pub const CURRENT_LEGACY_FILE_VERSION: i32 = -9;
    pub const OLDEST_SUPPORTED_LEGACY_FILE_VERSION: i32 = -2;
    pub const OLDEST_LOADABLE_UE4: i32 = 214;
    pub const LATEST_SUPPORTED_UE4: i32 = 522;
    pub const LATEST_SUPPORTED_UE5: i32 = 1018;

    #[must_use]
    pub const fn is_at_least_ue4(&self, version: i32) -> bool {
        self.ue4 >= version
    }

    #[must_use]
    pub const fn is_at_least_ue5(&self, version: i32) -> bool {
        self.ue5 >= version
    }

    #[must_use]
    pub const fn is_unversioned(&self) -> bool {
        self.ue4 == 0 && self.ue5 == 0 && self.licensee == 0
    }
}
