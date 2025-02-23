use rustc_version::{version, version_meta, Channel};
fn main() {
	// Assert we haven't travelled back in time
	let rustc_version = version().unwrap();
	let mut proj_version = semver::Version::parse(&std::env::var("CARGO_PKG_VERSION").unwrap_or_default()).unwrap();
	let deb_revision = std::env::var("DEB_REVISION").unwrap_or_default();
	if !deb_revision.is_empty() {
		if proj_version.build.is_empty() {
			proj_version.build = semver::BuildMetadata::new(&format!("build.{deb_revision}")).unwrap()
		} else {
			proj_version.build =
				semver::BuildMetadata::new(&format!("{}.build.{deb_revision}", proj_version.build)).unwrap()
		}
	}

	let profile = std::env::var("PROFILE").unwrap_or_default();
	let target = std::env::var("TARGET").unwrap_or_default();
	let target_feature = std::env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
	assert!(rustc_version.major >= 1);

	// Set cfg flags depending on release channel
	match version_meta().unwrap().channel {
		Channel::Stable => {
			println!("cargo::rustc-cfg=RUSTC_IS_STABLE");
		},
		Channel::Beta => {
			println!("cargo::rustc-cfg=RUSTC_IS_BETA");
		},
		Channel::Nightly => {
			println!("cargo::rustc-cfg=RUSTC_IS_NIGHTLY");
		},
		Channel::Dev => {
			println!("cargo::rustc-cfg=RUSTC_IS_DEV");
		},
	}
	//CARGO_CFG_TARGET_FEATURE
	println!("cargo::rustc-env=BUILD_PROFILE={profile}");
	println!("cargo::rustc-env=BUILD_TARGET={target}");
	println!("cargo::rustc-env=BUILD_TARGET_FEATURE={target_feature}");
	println!("cargo::rustc-env=RUSTC_VERSION={rustc_version}");

	let mut enabled_features = std::env::vars()
		.into_iter()
		.filter_map(|(key, value)| {
			if value != "1" || !key.starts_with("CARGO_FEATURE_") || key == "CARGO_FEATURE_DEFAULT" {
				None
			} else {
				Some(key["CARGO_FEATURE_".len()..].to_lowercase())
			}
		})
		.collect::<Vec<String>>();
	if std::env::var("CARGO_FEATURE_DEFAULT").unwrap_or_default().len() > 0 {
		enabled_features.insert(0, "default".into());
	}
	if enabled_features.is_empty() {
		enabled_features.push("(none)".into());
	}

	let enabled_features = enabled_features.join(",");
	println!("cargo::rustc-env=BUILD_FEATURE={enabled_features}");
	println!("cargo::rustc-env=BUILD_VERSION={proj_version}");
	println!(
		"cargo::rustc-env=BUILD_DATETIME={}",
		chrono::offset::Local::now().format("%Y-%m-%d %H:%M:%S (%Z)")
	);
	//println!("cargo::rustc-env=BUILD_REVISION={}", std::env::var("BUILD_REVISION").unwrap_or(std::env::var("DEB_REVISION")));
}
