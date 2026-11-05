mod categories;
mod commands;
mod favorites;
mod game_folders;
mod instance_cache;
mod mod_projects;
mod instances;
mod portable;
mod recycle;
mod settings;
mod single;
mod sweep;
mod version_meta;
mod versions;

use std::path::PathBuf;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// 启动器自身的运行信息，供前端展示"是否便携模式 / 数据放在哪"。
#[tauri::command]
fn launcher_info(paths: tauri::State<'_, portable::Paths>) -> serde_json::Value {
    serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "exeDir": paths.exe_dir().to_string_lossy(),
        "dataDir": paths.data_dir().to_string_lossy(),
        "gameDir": game_folders::current_string(&paths),
        "portable": paths.portable(),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 数据目录要先解析出来：单实例互斥键就是它（同一份数据只许一个进程，
    // 两份 exe 指向不同数据目录时仍可同时开）。
    let paths = match portable::Paths::resolve() {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("{error}");
            return;
        }
    };
    // 下载源策略是进程级状态：先按设置初始化，后面所有下载路径都按它走。
    commands::sync_global_strategy(&paths);
    if !single::acquire_or_focus(paths.data_dir()) {
        // 已有实例在跑：上面已经把它拉到前台，这里安静退出。
        return;
    }

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(commands::GameRuntime::default())
        .manage(commands::InstallRuntime::default())
        .invoke_handler(tauri::generate_handler![
            launcher_info,
            commands::instance_root,
        commands::list_entries,
        commands::read_text_file,
        commands::image_data_url,
        commands::read_servers,
        commands::export_instance_zip,
        commands::export_pack_layout,
        commands::export_pack_begin,
        commands::export_pack_identify,
        commands::export_pack_finish,
        commands::export_pack_pick_path,
        commands::export_pack_load_config,
        commands::export_pack_save_config,
        commands::export_pack_cancel,
        commands::list_versions,
            commands::save_version_settings,
            commands::delete_version,
            commands::open_version_folder,
            commands::default_game_dir,
            commands::ensure_dir,
            commands::scan_versions,
            commands::version_icon,
            commands::set_version_icon,
            commands::pick_version_icon,
            commands::set_version_pack,
            commands::set_version_description,
            commands::rename_instance,
            commands::reset_instance,
            commands::repair_instance_files,
            commands::diagnose_crash_report,
            commands::list_accounts,
            commands::add_offline_account,
            commands::rename_account,
            commands::remove_account,
            commands::select_account,
            commands::ms_login_start,
            commands::ms_login_poll,
            commands::refresh_account,
            commands::patch_instance_core,
            commands::backup_instance_core,
            commands::export_launch_script,
            commands::test_instance,
            commands::list_categories,
            commands::create_category,
            commands::rename_category,
            commands::delete_category,
            commands::set_version_categories,
            commands::set_starred,
            commands::import_starred,
            commands::check_java,
            commands::detect_java,
            commands::plan_java,
            commands::memory_info,
            commands::launch_version,
            commands::game_snapshot,
            commands::fetch_remote_versions,
            commands::fetch_fabric_loaders,
            commands::fetch_legacy_fabric_loaders,
            commands::fetch_labymod_versions,
            commands::fetch_quilt_loaders,
            commands::fetch_modloader_versions,
            commands::fetch_loader_catalog,
            commands::fetch_loader_dates,
            commands::install_game_version,
            commands::install_game_instance,
            commands::install_modpack,
            commands::install_game_modloader,
            commands::instance_name_taken,
            commands::cancel_install,
            commands::load_settings,
            commands::save_settings,
            commands::download_package,
            commands::save_package,
            commands::save_modrinth,
            commands::install_world,
            commands::save_world,
            commands::wiki_link,
            commands::curseforge_search,
            commands::curseforge_versions,
            commands::ftb_search,
            commands::ftb_versions,
            commands::install_ftb_modpack,
            commands::content_search,
            commands::content_categories,
            commands::fetch_icon,
            commands::modrinth_search,
            commands::modrinth_versions,
            commands::modrinth_browse,
            commands::modrinth_download,
            commands::favorites_load,
            commands::favorites_save,
            commands::read_launcher_log,
            commands::read_game_log,
            commands::launcher_log_path,
            commands::export_error_report,
            commands::list_mods,
            commands::set_mod_enabled,
            commands::remove_mod,
            commands::mod_icon,
            commands::mod_project,
            commands::check_mod_updates,
            commands::apply_mod_update,
            commands::add_mods,
            commands::dir_usage,
            commands::open_folder,
            game_folders::list_game_folders,
            game_folders::add_game_folder,
            game_folders::remove_game_folder,
            game_folders::rename_game_folder,
            game_folders::set_folder_order,
            game_folders::set_active_game_folder,
            game_folders::pick_game_folder,
            commands::reveal_path,
            commands::delete_output,
            commands::path_exists,
            commands::open_url,
            commands::cleanup_targets,
            commands::cleanup_run,
        ])
        .setup(move |app| {
            paths.write_startup_log(env!("CARGO_PKG_VERSION"));
            // 搜索/版本列表的 HTTP 缓存（对应 PCL 的 CacheCow），放在数据目录下。
            launcher_core::httpcache::init(paths.data_dir().join("http-cache"));
            // CurseForge 的 key 放在数据目录里，用户从不手填；没有就只用镜像。
            let key_file = paths.data_dir().join("curseforge.key");
            if let Ok(raw) = std::fs::read_to_string(&key_file) {
                launcher_core::curseforge::set_key(&raw);
            }
            // 旧的 instances.json 只在这时读一次，迁移完就改名留档。
            if let Err(error) = versions::migrate_legacy(paths.data_dir()) {
                eprintln!("migrate instances.json: {error}");
            }
            // 版本页左栏的顺序表要跟着实际扫出来的目录走。
            game_folders::remember_known(&paths);
            // 每条实例的设置现在是实例文件夹里的 `ybmcl.json`，老的整体存储搬过去。
            versions::migrate_store_into_instances(paths.data_dir(), &game_folders::current(&paths));

            // 老版本（以及别的启动器）留下的是「版本核心 + 加载器外壳」两层结构，
            // 启动时顺手并成自包含实例。只动 json 和 jar，mods/存档不动。
            let game_dir = game_folders::current(&paths);
            let sweep_dirs = game_folders::list(&paths)
                .into_iter()
                .map(|folder| PathBuf::from(folder.path))
                .collect::<Vec<_>>();
            // 上次硬杀留下的半成品先清掉，免得它们被当成「装了但缺文件」。
            // 放在建窗口之前同步做：清扫按「带标记就删」，跑起来以后再清会误删新装的实例。
            sweep::sweep_all(sweep_dirs);
            std::thread::spawn(move || {
                match launcher_core::merge::migrate_legacy_versions(&game_dir) {
                    Ok(ids) => {
                        for id in ids {
                            eprintln!("merged legacy instance: {id}");
                        }
                    }
                    Err(error) => eprintln!("merge legacy instances: {error}"),
                }
                match launcher_core::merge::prune_incomplete(&game_dir) {
                    Ok(removed) => {
                        for id in removed {
                            eprintln!("pruned unfinished instance: {id}");
                        }
                    }
                    Err(error) => eprintln!("prune unfinished instances: {error}"),
                }
                match launcher_core::merge::rename_legacy_versions(&game_dir) {
                    Ok(renames) => {
                        for (old, new) in renames {
                            eprintln!("renamed instance: {old} -> {new}");
                        }
                    }
                    Err(error) => eprintln!("rename legacy instances: {error}"),
                }
            });

            // 照 PCL 的 InstanceCache：启动时先把所有实例的概述算好写进缓存，
            // 之后打开版本页 / 实例页直接读缓存，不用再等「正在读取版本」。
            {
                let paths_warm = paths.clone();
                std::thread::spawn(move || {
                    let game_dir = game_folders::current(&paths_warm);
                    let data_dir = paths_warm.data_dir().to_owned();
                    // 给合并线程一点时间，别和小写盘抢同一个目录。
                    std::thread::sleep(std::time::Duration::from_millis(800));
                    commands::warm_instance_cache(&game_dir, &data_dir);
                });
            }

            // 窗口在代码里创建，唯一目的是把 WebView2 的用户数据目录
            // 强制指到 `YBUMCL/webview2`，否则它会写进 %LOCALAPPDATA%，
            // 便携性就破坏了。
            let webview_dir = paths.webview_dir();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title(portable::WINDOW_TITLE)
                .inner_size(1100.0, 720.0)
                .min_inner_size(960.0, 640.0)
                .center()
                .decorations(false)
                .data_directory(webview_dir)
                .build()?;

            app.manage(paths);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // 退出时把 HTTP 缓存删掉：它只用来加速同一次会话里的重复请求，
    // 留着只会越堆越多（PCL 也是把缓存放临时目录里随用随清）。
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            // 暂停过的半成品不等下次启动，关窗口就删（整合包的 `.ybmcl/modpack` 也在表里）。
            sweep::cleanup_paused_on_exit();
            if let Some(paths) = handle.try_state::<portable::Paths>() {
                let _ = std::fs::remove_dir_all(paths.data_dir().join("http-cache"));
            }
        }
    });
}
