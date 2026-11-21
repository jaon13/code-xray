use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::build_or_refresh_index;
use super::cs_extractor::extract_csharp_file;
use super::model::DiscoveredFile;
use super::open_db;
use super::slice_query::query_feature_slice;
use super::sqlite_query::{query_deps, query_partials, query_search, query_status};
use super::xaml_extractor::extract_xaml_file;

fn make_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("xray_index_test_{label}_{nanos}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn extracts_partial_di_messenger_enum_and_action_keys() {
    let sample = br#"
using CommunityToolkit.Mvvm.Messaging;
using Microsoft.Extensions.DependencyInjection;

namespace Demo.App;

public enum ActionKind { OcrClick, WaitMs }

public partial class StepDispatcher : IRecipient<StepRunMessage>
{
    public void Register(IServiceCollection services)
    {
        services.AddSingleton<IDispatcher, StepDispatcher>();
    }

    public string Resolve(string key) => key switch
    {
        "ACTION_RUN" => "ActionRun",
        _ => ""
    };

    public void SendNotice()
    {
        WeakReferenceMessenger.Default.Send(new StepDoneMessage());
    }
}
"#;
    let file = DiscoveredFile {
        project_name: "Demo.App".to_string(),
        abs_path: "StepDispatcher.cs".to_string(),
        rel_path: "src/Demo.App/StepDispatcher.cs".to_string(),
        lang: "csharp",
        size_bytes: sample.len() as i64,
        mtime_ns: 10,
    };

    let data = extract_csharp_file(&file, sample).expect("extract C#");
    assert!(data
        .types
        .iter()
        .any(|t| t.full_name == "Demo.App.StepDispatcher" && t.is_partial));
    assert!(data
        .members
        .iter()
        .any(|m| m.name == "OcrClick" && m.kind == "enum_member"));
    assert!(data
        .members
        .iter()
        .any(|m| m.name == "ACTION_RUN" && m.kind == "key_literal"));
    assert!(data.di_regs.iter().any(|d| d.lifetime == "Singleton"
        && d.service_type == "IDispatcher"
        && d.impl_type == "StepDispatcher"));
    assert!(data
        .messenger_links
        .iter()
        .any(|m| m.direction == "receive" && m.message_type == "StepRunMessage"));
    assert!(data
        .messenger_links
        .iter()
        .any(|m| m.direction == "send" && m.message_type == "StepDoneMessage"));
}

#[test]
fn extracts_xaml_class_and_datatype() {
    let xaml = br#"<UserControl x:Class="Demo.App.MainView"
        xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
        xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
        xmlns:vm="clr-namespace:Demo.App.ViewModels"
        d:DataContext="{d:DesignInstance Type=vm:MainViewModel}">
    <UserControl.Resources>
        <DataTemplate DataType="{x:Type vm:StepItemViewModel}" />
    </UserControl.Resources>
</UserControl>"#;
    let file = DiscoveredFile {
        project_name: "Demo.App".to_string(),
        abs_path: "MainView.xaml".to_string(),
        rel_path: "src/Demo.App/MainView.xaml".to_string(),
        lang: "xaml",
        size_bytes: xaml.len() as i64,
        mtime_ns: 5,
    };
    let data = extract_xaml_file(&file, xaml).expect("extract XAML");
    assert!(data
        .types
        .iter()
        .any(|t| t.full_name == "Demo.App.MainView" && t.kind == "xaml_view"));
    assert!(data
        .type_refs
        .iter()
        .any(|r| r.target_type == "MainViewModel"));
    assert!(data
        .type_refs
        .iter()
        .any(|r| r.target_type == "StepItemViewModel"));
}

#[test]
fn incremental_index_build_and_queries_work_end_to_end() {
    let root = make_temp_dir("e2e");
    let proj_dir = root.join("src").join("Demo.Core");
    fs::create_dir_all(&proj_dir).expect("mkdir");
    fs::write(
        root.join("Demo.slnx"),
        "<Solution><Project Path=\"src/Demo.Core/Demo.Core.csproj\" /></Solution>\n",
    )
    .expect("write slnx");
    fs::write(
        proj_dir.join("Demo.Core.csproj"),
        "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>\n",
    )
    .expect("write csproj");
    fs::write(
        proj_dir.join("Broker.Core.cs"),
        "namespace Demo.Core;\npublic partial class Broker { public string Tag => \"ACTION_RUN\"; }\n",
    )
    .expect("write part1");
    fs::write(
        proj_dir.join("Broker.Ops.cs"),
        "namespace Demo.Core;\npublic partial class Broker { public void Execute(Runner r) {} }\npublic class Runner {}\n",
    )
    .expect("write part2");

    let cold = build_or_refresh_index(&root, None, false, false).expect("cold build");
    assert_eq!(cold.parsed_files, 2);
    assert_eq!(cold.removed_files, 0);

    let warm = build_or_refresh_index(&root, None, false, false).expect("warm build");
    assert_eq!(warm.parsed_files, 0);
    assert_eq!(warm.skipped_unchanged, 2);

    let db_path = root.join(".xray").join("index.db");
    let conn = open_db(&db_path).expect("open db");
    let status = query_status(&conn).expect("status");
    assert_eq!(status.files, 2);

    let partials = query_partials(&conn, Some("Broker"), 2).expect("partials");
    assert_eq!(partials.len(), 1);
    assert_eq!(partials[0].file_count, 2);

    let slice = query_feature_slice(&conn, "ACTION_RUN", 2, 10).expect("slice");
    assert!(slice
        .files_to_read
        .iter()
        .any(|f| f.file.contains("Broker.Core.cs")));

    let deps = query_deps(&conn, "Broker").expect("deps");
    assert_eq!(deps.target, "Broker");

    let hits = query_search(&conn, "ACTION_RUN", None, 10).expect("search");
    assert!(!hits.is_empty());

    drop(conn);
    fs::remove_file(proj_dir.join("Broker.Ops.cs")).expect("remove part2");
    let after_del = build_or_refresh_index(&root, None, false, false).expect("after delete");
    assert_eq!(after_del.removed_files, 1);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn concurrent_builds_serialize_via_build_lock_without_duplicate_parse() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let root = make_temp_dir("concurrent_threads");
    let proj_dir = root.join("src").join("Demo.App");
    fs::create_dir_all(&proj_dir).expect("mkdir");
    fs::write(
        root.join("Demo.slnx"),
        "<Solution><Project Path=\"src/Demo.App/Demo.App.csproj\" /></Solution>\n",
    )
    .expect("write slnx");
    fs::write(
        proj_dir.join("Demo.App.csproj"),
        "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>\n",
    )
    .expect("write csproj");
    for i in 0..8 {
        fs::write(
            proj_dir.join(format!("Service{i}.cs")),
            format!(
                "namespace Demo.App;\npublic sealed class Service{i} {{ public int Run() => {i}; }}\n"
            ),
        )
        .expect("write cs");
    }

    let workers = 6usize;
    let barrier = Arc::new(Barrier::new(workers));
    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let root_c = root.clone();
        let barrier_c = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier_c.wait();
            build_or_refresh_index(&root_c, None, false, false)
        }));
    }

    let mut summaries = Vec::with_capacity(workers);
    for h in handles {
        let res = h.join().expect("thread join").expect("build ok");
        summaries.push(res);
    }

    // Exactly one builder parses all 8 files; all subsequent waiters skip unchanged files.
    let parsed_total: usize = summaries.iter().map(|s| s.parsed_files).sum();
    assert_eq!(parsed_total, 8);
    assert_eq!(summaries.iter().filter(|s| s.parsed_files == 8).count(), 1);
    assert!(root.join(".xray").join("index.build.lock").is_file());

    let conn = open_db(&root.join(".xray").join("index.db")).expect("open db");
    let status = query_status(&conn).expect("status");
    assert_eq!(status.files, 8);
    drop(conn);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn extracts_csharp_state_transitions_and_switch_arms() {
    let sample = br#"
namespace Demo.App;

public enum RunState { Idle, Running, Paused, Alarm }

public sealed class WaferRunner
{
    private RunState _state = RunState.Idle;

    public void Start()
    {
        if (_state == RunState.Idle)
        {
            _state = RunState.Running;
        }
    }

    public void Pause()
    {
        switch (_state)
        {
            case RunState.Running:
                _state = RunState.Paused;
                break;
        }
    }

    public void HandleFault()
    {
        _state = _state switch
        {
            RunState.Paused => RunState.Alarm,
            _ => RunState.Alarm,
        };
    }
}
"#;
    let file = DiscoveredFile {
        project_name: "Demo.App".to_string(),
        rel_path: "WaferRunner.cs".to_string(),
        abs_path: "WaferRunner.cs".to_string(),
        lang: "cs",
        mtime_ns: 0,
        size_bytes: sample.len() as i64,
    };
    let extracted = extract_csharp_file(&file, sample).expect("extract cs states");
    let pairs: Vec<(&str, &str, &str, &str)> = extracted
        .state_transitions
        .iter()
        .map(|s| {
            (
                s.machine_name.as_str(),
                s.from_state.as_str(),
                s.to_state.as_str(),
                s.trigger_member.as_str(),
            )
        })
        .collect();
    assert!(pairs.contains(&("RunState", "(init)", "Idle", "<init>")));
    assert!(pairs.contains(&("RunState", "Idle", "Running", "Start")));
    assert!(pairs.contains(&("RunState", "Running", "Paused", "Pause")));
    assert!(pairs.contains(&("RunState", "Paused", "Alarm", "HandleFault")));
}

#[test]
fn upgrading_v1_schema_reindexes_and_detects_orphan_terminal_states() {
    let root = make_temp_dir("state_v1_upgrade");
    let proj_dir = root.join("src").join("Demo.App");
    fs::create_dir_all(&proj_dir).expect("mkdir");
    fs::write(
        root.join("Demo.slnx"),
        "<Solution><Project Path=\"src/Demo.App/Demo.App.csproj\" /></Solution>\n",
    )
    .expect("slnx");
    fs::write(
        proj_dir.join("Demo.App.csproj"),
        "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>\n",
    )
    .expect("csproj");
    fs::write(
        proj_dir.join("Runner.cs"),
        "namespace Demo.App;\npublic enum RunState { Idle, Running, Alarm }\npublic sealed class Runner {\n    private RunState _state = RunState.Idle;\n    public void Run() { if (_state == RunState.Idle) { _state = RunState.Running; } }\n    public void Trip() { if (_state == RunState.Running) { _state = RunState.Alarm; } }\n}\n",
    )
    .expect("cs");

    let first = build_or_refresh_index(&root, None, false, false).expect("initial build");
    assert!(first.total_state_transitions >= 3);

    // Simulate legacy schema_version = 1 index with empty state_transitions table.
    let db_path = root.join(".xray").join("index.db");
    let conn = open_db(&db_path).expect("open db");
    conn.execute(
        "UPDATE meta SET value = '1' WHERE key = 'schema_version'",
        [],
    )
    .expect("downgrade meta");
    conn.execute("DELETE FROM state_transitions", [])
        .expect("clear states");
    drop(conn);

    let upgraded = build_or_refresh_index(&root, None, false, false).expect("upgrade build");
    assert_eq!(upgraded.parsed_files, 1);
    assert!(upgraded.total_state_transitions >= 3);

    let conn2 = open_db(&db_path).expect("reopen");
    let states = super::sqlite_query::query_states(&conn2, Some("Runner")).expect("query_states");
    assert_eq!(states.machines, vec!["RunState".to_string()]);
    assert!(states
        .orphan_terminal_states
        .contains(&"RunState.Alarm".to_string()));
    assert!(!states
        .orphan_terminal_states
        .contains(&"RunState.Running".to_string()));
    drop(conn2);

    let _ = fs::remove_dir_all(&root);
}
