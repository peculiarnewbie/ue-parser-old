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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_flags_preserve_bits_and_detect_individual_capabilities() {
        let flags =
            PackageFlags::from_bits(PackageFlags::COOKED | PackageFlags::FILTER_EDITOR_ONLY);

        assert_eq!(
            flags.bits(),
            PackageFlags::COOKED | PackageFlags::FILTER_EDITOR_ONLY
        );
        assert!(flags.contains(PackageFlags::COOKED));
        assert!(flags.contains(PackageFlags::FILTER_EDITOR_ONLY));
        assert!(!flags.contains(PackageFlags::UNVERSIONED_PROPERTIES));
    }

    #[test]
    fn version_predicates_include_the_boundary() {
        let versions = VersionContext {
            ue4: 522,
            ue5: 1018,
            licensee: 0,
            ..VersionContext::default()
        };

        assert!(versions.is_at_least_ue4(522));
        assert!(!versions.is_at_least_ue4(523));
        assert!(versions.is_at_least_ue5(1018));
        assert!(!versions.is_at_least_ue5(1019));
        assert!(!versions.is_unversioned());

        assert!(VersionContext::default().is_unversioned());
        assert!(
            !VersionContext {
                licensee: 1,
                ..VersionContext::default()
            }
            .is_unversioned()
        );
    }
}
