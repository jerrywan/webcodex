use super::*;

#[test]
fn runtime_command_inherits_only_tunnel_authority_not_server_bootstrap_or_openai_admin_keys() {
    let prerequisites = OpenAiTunnelPrerequisites {
        binary: PathBuf::from("tunnel-client"),
        tunnel_id: "tunnel_0123456789abcdef0123456789abcdef".to_string(),
    };
    let mut command = Command::new("tunnel-client");
    configure_runtime_command(
        &mut command,
        &prerequisites,
        "http://127.0.0.1:8080/mcp",
        Path::new("authorization"),
    );

    let env = command.as_std().get_envs().collect::<Vec<_>>();
    for key in ["WEBCODEX_TOKEN", "OPENAI_ADMIN_KEY", "OPENAI_API_KEY"] {
        assert!(env
            .iter()
            .any(|(name, value)| { name.to_str() == Some(key) && value.is_none() }));
    }
    assert!(env.iter().any(|(name, value)| {
        name.to_str() == Some("CONTROL_PLANE_TUNNEL_ID")
            && value.and_then(|value| value.to_str()) == Some(prerequisites.tunnel_id.as_str())
    }));
}

#[test]
fn tunnel_ids_are_strict_and_runtime_key_never_part_of_the_id_contract() {
    assert!(valid_tunnel_id("tunnel_0123456789abcdef0123456789abcdef"));
    for invalid in [
        "0123456789abcdef0123456789abcdef",
        "tunnel_0123456789ABCDEF0123456789ABCDEF",
        "tunnel_0123456789abcdef",
        "tunnel_0123456789abcdef0123456789abcdef0",
        "tunnel_0123456789abcdef0123456789abcdeg",
    ] {
        assert!(
            !valid_tunnel_id(invalid),
            "accepted invalid tunnel id {invalid}"
        );
    }
}

#[test]
fn official_release_assets_and_extracted_binaries_are_pinned_per_supported_platform() {
    let linux_amd64 = tunnel_client_asset_for("linux", "x86_64").unwrap();
    assert_eq!(
        linux_amd64.file_name,
        "tunnel-client-v0.0.14-linux-amd64.zip"
    );
    assert_eq!(
        linux_amd64.archive_sha256,
        "15bd17e805cad39d412199115bb9e10a978dd35258a114cdf25dd2ae6681c7d3"
    );
    assert_eq!(
        linux_amd64.binary_sha256,
        "472eb9dd9dd625b4e6023c3b4a5736b3a2e5a1b6dbe9338e001887a64ec992a6"
    );

    let linux_arm64 = tunnel_client_asset_for("linux", "aarch64").unwrap();
    assert_eq!(
        linux_arm64.binary_sha256,
        "ab6c05258f15dc43a8e23f39460beb69892a8ced03e4c345a6f1aef0dd009b0f"
    );
    let darwin_amd64 = tunnel_client_asset_for("macos", "x86_64").unwrap();
    assert_eq!(
        darwin_amd64.binary_sha256,
        "89478d1d58350818275b852169745e1af0e18c02ff9b5b46d50df22018c95be9"
    );
    let darwin_arm64 = tunnel_client_asset_for("macos", "aarch64").unwrap();
    assert_eq!(
        darwin_arm64.binary_sha256,
        "309fd85da5a8c2ca8dae920deea8ac10a4d7934ed18ac46e7df0c200139cc9c5"
    );
    assert_eq!(darwin_arm64.target, "darwin-arm64");

    let windows_amd64 = tunnel_client_asset_for("windows", "x86_64").unwrap();
    assert_eq!(windows_amd64.target, "windows-amd64");
    assert_eq!(windows_amd64.member_name, "tunnel-client.exe");
    assert_eq!(
        windows_amd64.archive_sha256,
        "784ab8da7b5a88f0109f1fd8aaf0a1c86067430b896dddf307ef7e3cc49fa1a5"
    );
    assert_eq!(
        windows_amd64.binary_sha256,
        "fcc85a69ec0ad82518e4f8964f60c45e31787957782a0fc9c1b0c44e82d61b9b"
    );
    let windows_arm64 = tunnel_client_asset_for("windows", "aarch64").unwrap();
    assert_eq!(windows_arm64.target, "windows-arm64");
    assert_eq!(windows_arm64.member_name, "tunnel-client.exe");
    assert_eq!(
        windows_arm64.archive_sha256,
        "fa775db8897df543dd4ba66404f69492a2acfbc6a291f10df27aced064a16568"
    );
    assert_eq!(
        windows_arm64.binary_sha256,
        "7260ec886a7efd34202c6506bd35b068e94723a5402ea6f76af5a3af3dbd0a0b"
    );
}

#[test]
fn tunnel_output_is_reduced_to_safe_diagnostic_categories() {
    assert_eq!(
        tunnel_output_hint(b"", b"HTTP 401 unauthorized"),
        "Runtime API key authentication was rejected"
    );
    assert_eq!(
        tunnel_output_hint(b"", b"403 forbidden"),
        "Runtime API key does not have the required Tunnel permission"
    );
    assert_eq!(
        tunnel_output_hint(b"", b"proxy connect failed"),
        "proxy configuration or proxy connectivity failed"
    );
    assert_eq!(
        tunnel_output_hint(b"", b"completely unknown failure"),
        "tunnel-client returned a non-success status"
    );
}

#[test]
fn managed_root_prefers_private_xdg_then_home() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let home = temp.path().join("home");
    let local = temp.path().join("local");
    assert_eq!(
        managed_tunnel_client_root_from(
            Some(state.as_os_str()),
            Some(home.as_os_str()),
            Some(local.as_os_str()),
        )
        .unwrap(),
        state.join("webcodex/tools/tunnel-client")
    );
    assert_eq!(
        managed_tunnel_client_root_from(None, Some(home.as_os_str()), Some(local.as_os_str()),)
            .unwrap(),
        home.join(".local/state/webcodex/tools/tunnel-client")
    );
    assert_eq!(
        managed_tunnel_client_root_from(None, None, Some(local.as_os_str())).unwrap(),
        local.join("WebCodex/tools/tunnel-client")
    );
    assert!(managed_tunnel_client_root_from(None, None, None).is_err());
    assert!(managed_tunnel_client_root_from(
        Some(OsStr::new("relative")),
        Some(home.as_os_str()),
        None,
    )
    .is_err());
}

#[test]
fn health_url_accepts_only_bounded_loopback_http_origins() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("health.url");
    for valid in [
        "http://127.0.0.1:12345\n",
        "http://localhost:43210/\n",
        "http://[::1]:3000\n",
    ] {
        fs::write(&path, valid).unwrap();
        let result = read_loopback_health_url(&path).unwrap();
        assert!(result.starts_with("http://"));
    }
    for invalid in [
        "https://127.0.0.1:12345",
        "http://example.com:12345",
        "http://127.0.0.1",
        "http://user:secret@127.0.0.1:12345",
        "http://127.0.0.1:12345/readyz",
        "http://127.0.0.1:12345?secret=value",
    ] {
        fs::write(&path, invalid).unwrap();
        assert!(
            read_loopback_health_url(&path).is_err(),
            "accepted {invalid}"
        );
    }
}

#[test]
fn zip_extraction_reads_only_the_exact_tunnel_client_member() {
    let temp = tempfile::tempdir().unwrap();
    let archive_path = temp.path().join("client.zip");
    let file = File::create(&archive_path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    archive.start_file("../tunnel-client", options).unwrap();
    archive.write_all(b"wrong").unwrap();
    archive.start_file("tunnel-client", options).unwrap();
    archive.write_all(b"expected-binary").unwrap();
    archive.finish().unwrap();

    let destination = temp.path().join("extracted");
    extract_tunnel_client(&archive_path, &destination, "tunnel-client").unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"expected-binary");
    assert!(!temp.path().join("tunnel-client").exists());

    let windows_archive_path = temp.path().join("windows-client.zip");
    let file = File::create(&windows_archive_path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    archive.start_file("../tunnel-client.exe", options).unwrap();
    archive.write_all(b"wrong-windows").unwrap();
    archive.start_file("tunnel-client.exe", options).unwrap();
    archive.write_all(b"expected-windows-binary").unwrap();
    archive.finish().unwrap();
    let windows_destination = temp.path().join("extracted.exe");
    extract_tunnel_client(
        &windows_archive_path,
        &windows_destination,
        "tunnel-client.exe",
    )
    .unwrap();
    assert_eq!(
        fs::read(&windows_destination).unwrap(),
        b"expected-windows-binary"
    );
    assert!(!temp.path().join("tunnel-client.exe").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn version_verification_requires_the_pinned_client_line() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let good = temp.path().join("good");
    fs::write(&good, "#!/bin/sh\necho '0.0.14+test (git sha: abc)'\n").unwrap();
    fs::set_permissions(&good, fs::Permissions::from_mode(0o700)).unwrap();
    verify_tunnel_client_version(&good).await.unwrap();

    let wrong = temp.path().join("wrong");
    fs::write(&wrong, "#!/bin/sh\necho '0.0.13'\n").unwrap();
    fs::set_permissions(&wrong, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(verify_tunnel_client_version(&wrong).await.is_err());
}
