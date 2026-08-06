export type InspectStatus = "ok" | "partial";

export type TableCounts = {
  count: number;
  offset?: number;
};

export type PropertyValue =
  | { value_kind: "bool"; value: boolean }
  | { value_kind: "int"; value: number }
  | { value_kind: "uint"; value: number }
  | { value_kind: "float"; value: number }
  | { value_kind: "double"; value: number }
  | { value_kind: "name"; value: string }
  | { value_kind: "enum"; value: string }
  | { value_kind: "string"; value: string }
  | { value_kind: "text"; value: string }
  | { value_kind: "vector"; x: number; y: number; z: number }
  | { value_kind: "object_ref"; value: string | null }
  | { value_kind: "guid"; value: string }
  | { value_kind: "soft_object_path"; value: string }
  | { value_kind: "array"; values: PropertyValue[] }
  | { value_kind: "set"; values: PropertyValue[] }
  | { value_kind: "map"; entries: { key: PropertyValue; value: PropertyValue }[] }
  | { value_kind: "struct"; properties: UassetProperty[] }
  | { value_kind: "raw"; reason: string; size: number };

export type UassetProperty = {
  name: string;
  type: string;
} & PropertyValue;

export type DataTableRowJson = {
  name: string;
  properties: UassetProperty[];
};

export type AssetSummary = {
  kind: string;
  object_path: string;
  class_path?: string;
  object_guid?: string;
  row_struct?: string;
  parent_tables?: string[];
  row_count: number;
  properties?: UassetProperty[];
  enum_entries?: unknown[];
  struct_fields?: unknown[];
  tail_bytes?: number;
  rows?: DataTableRowJson[];
};

export type UassetInspect = {
  schema_version: number;
  status: InspectStatus;
  path: string;
  package: {
    name: string;
    version: {
      legacy_file: number;
      legacy_ue3: number;
      ue4: number;
      ue5: number;
      licensee: number;
    };
    package_flags: number;
    summary_size: number;
    total_header_size: number;
    names: TableCounts;
    imports: TableCounts;
    exports: TableCounts;
    soft_object_paths?: { count: number; parsed_count: number };
  };
  assets: AssetSummary[];
  decode_errors?: {
    object_path: string;
    class_path?: string;
    kind: string;
    message: string;
  }[];
};

export type CpuScopeSummary = {
  spec_id: number;
  name: string;
  count: number;
  total_cycles: number;
  total_seconds?: number;
};

export type GpuFrameBreadcrumbSummary = {
  name: string;
  count: number;
  total_cycles: number;
};

export type CorrelatedFrameSummary = {
  frame_number: number;
  cpu_metadata_count: number;
  cpu_metadata_cycles: number;
  cpu_metadata_seconds?: number;
  cpu_begin_cycle?: number;
  cpu_end_cycle?: number;
  top_cpu_scopes?: CpuScopeSummary[];
  gpu_queue_count: number;
  gpu_work_count: number;
  gpu_work_cycles: number;
  gpu_breadcrumb_count: number;
  gpu_breadcrumb_cycles: number;
  top_gpu_breadcrumbs?: GpuFrameBreadcrumbSummary[];
};

export type FrameTimingSummary = {
  frame_number: number;
  frame_type: number;
  begin_cycle: number;
  end_cycle: number;
  duration_cycles: number;
  duration_seconds?: number;
  gpu_submitted_work_count: number;
  gpu_submitted_work_cycles: number;
};

export type CpuTimelineInterval = {
  thread_id: number;
  spec_id: number;
  name: string;
  start_cycle: number;
  end_cycle: number;
  duration: number;
  duration_seconds?: number;
  metadata_id?: number;
  rendered_name?: string;
};

export type CpuTimelineDashboard = {
  frame_number: number;
  begin_cycle: number;
  end_cycle: number;
  duration_seconds?: number;
  interval_count: number;
  truncated: boolean;
  intervals: CpuTimelineInterval[];
};

export type UtraceTimelineQuery = {
  schema_version: number;
  status: InspectStatus;
  path: string;
  timeline: {
    index: {
      source_bytes: number;
      source_fingerprint: number;
      cycle_frequency?: number;
      total_interval_count: number;
      indexed_interval_count: number;
      truncated: boolean;
      begin_cycle?: number;
      end_cycle?: number;
    };
    begin_cycle: number;
    end_cycle: number;
    duration_seconds?: number;
    interval_count: number;
    truncated: boolean;
    intervals: CpuTimelineInterval[];
  };
};

export type GpuTimelineInterval = {
  queue_id: number;
  kind: "work" | "breadcrumb";
  spec_id?: number;
  name: string;
  start_timestamp: number;
  end_timestamp: number;
  duration: number;
};

export type GpuTimelineDashboard = {
  frame_number: number;
  begin_timestamp: number;
  end_timestamp: number;
  interval_count: number;
  truncated: boolean;
  intervals: GpuTimelineInterval[];
};

export type UtraceGpuTimelineQuery = {
  schema_version: number;
  status: InspectStatus;
  path: string;
  timeline: GpuTimelineDashboard;
};

export type SampleValue =
  | number
  | string
  | { kind: string; byte_len: number; hex_prefix?: string };

export type EventSample = {
  thread_id: number;
  fields: Record<string, SampleValue>;
};

export type CounterSamplePoint = { cycle: number; value: number };
export type StatSamplePoint = { cycle: number; value: number };

export type CounterSummary = {
  id: number;
  name: string;
  kind: "int" | "float" | "unknown";
  display_hint: "none" | "memory" | "unknown";
  samples: number;
  first_cycle?: number;
  last_cycle?: number;
  min?: number;
  max?: number;
  latest?: number;
  sample_points?: CounterSamplePoint[];
};

export type StatSampleSummary = {
  id: number;
  samples: number;
  first_cycle?: number;
  last_cycle?: number;
  min?: number;
  max?: number;
  latest?: number;
  sample_points?: StatSamplePoint[];
};

export type SessionInfo = {
  platform: string;
  app_name: string;
  project_name: string;
  command_line: string;
  branch: string;
  build_version: string;
  changelist: number;
  configuration: string;
  target_type: string;
  instance_id?: string;
  vfs_paths?: string[];
};

export type TraceDashboardBody = {
  header: { magic: string; transport: number; protocol: number };
  prologue?: {
    start_cycle: number;
    cycle_frequency: number;
    endian: number;
    pointer_size: number;
    start_date_time: number;
  };
  thread_info?: {
    thread_id: number;
    system_id: number;
    sort_hint: number;
    name: string;
    groups?: string[];
    active_group?: string;
  }[];
  cpu: {
    specs?: { id: number; name: string; file?: string; line?: number }[];
    metadata?: {
      specs: number;
      records: number;
      scopes: number;
      resolved_scopes: number;
      unresolved_scopes: number;
      top?: { spec_id: number; name: string; count: number; total_cycles: number }[];
      rendered_scopes?: {
        spec_id: number;
        name: string;
        rendered_name: string;
        count: number;
        total_cycles: number;
      }[];
    };
    batches?: {
      count: number;
      decoded_records: number;
      intervals: number;
      unresolved_specs: number;
      metadata_scopes: number;
      restored_metadata_scopes: number;
      coroutine_records: number;
      unmatched_ends: number;
      unterminated_scopes: number;
      preamble_timeline_rebases?: number;
      implausible_duration_count?: number;
      implausible_duration_cycles?: number;
    };
    scopes: CpuScopeSummary[];
    threads: {
      thread_id: number;
      system_id?: number;
      name?: string;
      groups?: string[];
      active_group?: string;
      count: number;
      total_cycles: number;
      total_seconds?: number;
      scopes?: CpuScopeSummary[];
    }[];
    named_events?: {
      event: string;
      observed_count: number;
      sample?: EventSample;
    }[];
    end_threads?: { thread_id: number; cycle: number }[];
    timeline?: CpuTimelineDashboard;
  };
  frame_timing?: {
    total_frame_count: number;
    frames: FrameTimingSummary[];
  };
  gpu: {
    version?: number;
    queues: {
      queue_id: number;
      gpu: number;
      index: number;
      queue_type: number;
      name?: string;
      work_count: number;
      work_total_cycles: number;
      wait_count: number;
      wait_total_cycles: number;
      frame_boundary_count: number;
      last_frame_number?: number;
      draw_count: number;
      primitive_count: number;
      signal_fence_count: number;
      wait_fence_count: number;
      breadcrumb_count: number;
      breadcrumb_total_cycles: number;
      unmatched_work_ends?: number;
      unterminated_work?: number;
      unmatched_breadcrumb_ends?: number;
      unterminated_breadcrumbs?: number;
    }[];
    frames: {
      queue_id: number;
      frame_number: number;
      boundary_count: number;
      work_count: number;
      work_total_cycles: number;
      breadcrumb_count: number;
      breadcrumb_total_cycles: number;
      wait_count: number;
      wait_total_cycles: number;
      draw_count: number;
      primitive_count: number;
      top_breadcrumbs?: GpuFrameBreadcrumbSummary[];
    }[];
    total_frame_count: number;
    frames_truncated: boolean;
    timeline?: GpuTimelineDashboard;
    work?: {
      queues: number;
      intervals: number;
      total_cycles: number;
      unmatched_ends: number;
      negative_durations: number;
      unterminated_scopes: number;
    };
    breadcrumbs?: {
      specs: number;
      intervals: number;
      total_cycles: number;
      top?: {
        spec_id: number;
        name: string;
        count: number;
        total_cycles: number;
        metadata_events: number;
        metadata_bytes: number;
      }[];
    };
    submission_latency?: {
      sample_count: number;
      median_delay_cycles: number;
      min_delay_cycles: number;
      max_delay_cycles: number;
      samples?: {
        queue_id: number;
        gpu_timestamp_top: number;
        cpu_submit_timestamp: number;
        delay_cycles: number;
      }[];
    };
  };
  counters: {
    specs: number;
    counters: CounterSummary[];
    samples: number;
    int_samples: number;
    float_samples: number;
    unresolved_samples: number;
  };
  stats: {
    specs: number;
    floating_point_specs: number;
    memory_specs: number;
    clear_every_frame_specs: number;
    sample_events: number;
    unresolved_samples?: number;
    sample_state_overflow?: number;
    malformed_batches?: number;
    groups: {
      name: string;
      specs: number;
      floating_point_specs: number;
      memory_specs: number;
      clear_every_frame_specs: number;
    }[];
    stats: {
      id: number;
      name: string;
      description: string;
      group: string;
      is_floating_point: boolean;
      is_memory: boolean;
      should_clear_every_frame: boolean;
    }[];
    samples?: StatSampleSummary[];
  };
  csv: {
    categories: number;
    stats: number;
    declared_stats: number;
    inline_stats: number;
    unresolved_stats: number;
    sample_events: number;
    begin_events?: number;
    end_events?: number;
    custom_int_samples?: number;
    custom_float_samples?: number;
    top_categories: {
      index: number;
      name: string;
      stats: number;
      declared_stats: number;
      inline_stats: number;
    }[];
    stat_defs: {
      stat_id: number;
      name: string;
      category_index: number;
      category?: string;
      kind: "declared" | "inline";
    }[];
    duration_samples?: {
      thread_id: number;
      stat_id: number;
      begin_cycle: number;
      end_cycle: number;
      duration_cycles: number;
    }[];
    value_samples?: {
      thread_id: number;
      stat_id: number;
      cycle: number;
      value: number;
      op_type: number;
      kind: string;
    }[];
  };
  loading: {
    class_count: number;
    classes: { class: number; name: string }[];
    package_count: number;
    packages: {
      async_package: number;
      name: string;
      total_header_size: number;
      import_count: number;
      export_count: number;
      priority?: number;
    }[];
    requests: {
      begun: number;
      ended: number;
      completed: number;
      unmatched_ends: number;
      open: number;
      total_cycles: number;
      samples: {
        request_id: number;
        start_cycle: number;
        end_cycle: number;
        duration_cycles: number;
      }[];
    };
    async_loading: {
      starts: number;
      suspends: number;
      resumes: number;
      first_cycle?: number;
      last_cycle?: number;
    };
  };
  io_store: {
    backend_count: number;
    backends: { backend_handle: number; name: string; starts: number }[];
    requests_created: number;
    requests_started: number;
    requests_completed: number;
    requests_failed: number;
    requests_unresolved: number;
    bytes_requested: number;
    bytes_completed: number;
    request_samples: {
      request_handle: number;
      batch_handle: number;
      chunk_id_hash: number;
      chunk_type: number;
      offset: number;
      size: number;
      backend_handle?: number;
      backend_name?: string;
      create_cycle: number;
      start_cycle?: number;
      complete_cycle?: number;
      completed_size?: number;
      status: string;
    }[];
  };
  platform_file: {
    file_count: number;
    file_overflow?: number;
    files: {
      path: string;
      opens: number;
      open_failures: number;
      reopens: number;
      closes: number;
      reads: number;
      writes: number;
      bytes_read: number;
      bytes_written: number;
      bytes_requested_read: number;
      bytes_requested_write: number;
    }[];
    opens: number;
    open_failures: number;
    reopens: number;
    closes: number;
    reads: number;
    writes: number;
    bytes_read: number;
    bytes_written: number;
    bytes_requested_read: number;
    bytes_requested_write: number;
    activity_samples: {
      kind: string;
      path?: string;
      thread_id: number;
      file_handle?: number;
      offset?: number;
      size?: number;
      actual_size?: number;
      start_cycle: number;
      end_cycle?: number;
      duration_cycles?: number;
      failed: boolean;
    }[];
  };
  memory: {
    init?: {
      version: number;
      page_size: number;
      marker_period: number;
      min_alignment: number;
      size_shift: number;
    };
    tag_count: number;
    tag_overflow?: number;
    tags: { tag: number; parent: number; display: string }[];
    scope_count: number;
    scopes: { tag: number; count: number; display?: string }[];
    allocs: {
      count: number;
      free_count: number;
      realloc_alloc_count: number;
      realloc_free_count: number;
      bytes_allocated: number;
      bytes_freed: number;
      net_bytes: number;
      unresolved_free: number;
      outstanding_allocations: number;
      outstanding_bytes: number;
      outstanding_overflow: boolean;
      outstanding_dropped?: number;
      by_root_heap: {
        root_heap: number;
        name: string;
        alloc_count: number;
        free_count: number;
        bytes_allocated: number;
        bytes_freed: number;
        net_bytes: number;
      }[];
      samples?: {
        address: number;
        size: number;
        root_heap: number;
        callstack_id: number;
        callstack: string;
        kind: string;
      }[];
    };
    llm: {
      tag_count: number;
      tracker_count: number;
      tag_set_count: number;
      sample_events: number;
      tags?: { tag: number; parent: number; tag_set: number; name: string }[];
      trackers?: { tracker_id: number; name: string }[];
      latest_values?: {
        tracker_id: number;
        cycle: number;
        tag: number;
        value: number;
      }[];
      latest_values_overflow?: boolean;
    };
  };
  callstacks: {
    observed: number;
    retained: number;
    dropped: number;
    truncated: boolean;
    duplicate_ids: number;
    malformed: number;
    total_frames_retained: number;
    unresolved_references?: number;
    stacks?: {
      id: number;
      frame_count: number;
      frames_truncated?: boolean;
      frames: string[];
      mapped_frames?: {
        address: string;
        module?: string;
        relative_address?: string;
        symbol?: string;
        file?: string;
        line?: number;
        status: string;
      }[];
    }[];
  };
  modules: {
    init_seen: boolean;
    missing_init: boolean;
    symbol_format?: string;
    module_base_shift: number;
    observed_loads: number;
    observed_unloads: number;
    retained: number;
    dropped: number;
    truncated: boolean;
    modules?: {
      name: string;
      base: string;
      size: number;
      image_id_hex?: string;
      identity?: { guid: string; age: number };
      unloaded: boolean;
    }[];
  };
  metadata_stack: {
    clear_scope_count: number;
    saved_stack_count: number;
    restored_stack_count: number;
    unmatched_restore_count: number;
  };
  slate: {
    added_widgets: number;
    widgets: {
      widget_id: number;
      count: number;
      first_cycle?: number;
      last_cycle?: number;
    }[];
  };
  channels: {
    count: number;
    enabled: number;
    read_only: number;
    toggles: number;
    channels: {
      id: number;
      name?: string;
      is_enabled: boolean;
      read_only: boolean;
      toggle_count: number;
    }[];
  };
  thread_groups: {
    begin_events: number;
    end_events: number;
    unmatched_ends: number;
    unclosed_groups: number;
    groups: {
      name: string;
      begin_count: number;
      end_count: number;
      balanced: boolean;
    }[];
  };
  tasks: {
    init_version?: number;
    created: number;
    launched: number;
    scheduled: number;
    started: number;
    finished: number;
    completed: number;
    destroyed: number;
    subsequent_added?: number;
    wait_count: number;
    wait_started?: number;
    wait_finished?: number;
    unmatched_wait_ends?: number;
    open_waits?: number;
    wait_samples?: {
      thread_id: number;
      start_cycle: number;
      end_cycle: number;
      duration_cycles: number;
      task_ids?: number[];
    }[];
    named_tasks?: { task_id: number; debug_name: string }[];
  };
  annotations: {
    bookmarks: {
      specs: number;
      events: number;
      format_args_bytes: number;
      unresolved_events: number;
      bookmarks: {
        bookmark_point: number;
        format_string: string;
        file?: string;
        line?: number;
        count: number;
        format_args_bytes: number;
        sample_args?: string[];
        sample_message?: string;
        first_cycle?: number;
        last_cycle?: number;
        callstack_count: number;
        callstack_samples?: {
          cycle: number;
          callstack_id: number;
          callstack: string;
        }[];
      }[];
    };
    regions: {
      begin_events: number;
      end_events: number;
      completed: number;
      unmatched_ends: number;
      unterminated: number;
      regions: {
        name: string;
        category?: string;
        count: number;
        total_cycles: number;
      }[];
    };
  };
  logging: {
    categories: number;
    message_specs: number;
    messages: number;
    format_args_bytes: number;
    unresolved_messages: number;
    verbosity: { verbosity: string; message_specs: number; messages: number }[];
    top_categories: {
      name: string;
      default_verbosity: string;
      message_specs: number;
      messages: number;
    }[];
    top_messages: {
      log_point: number;
      category?: string;
      verbosity: string;
      format_string: string;
      file?: string;
      line?: number;
      count: number;
      sample_message?: string;
      first_cycle?: number;
      last_cycle?: number;
    }[];
  };
  unmodeled: {
    event_types: number;
    observed_events: number;
    events: {
      logger: string;
      event: string;
      observed_count: number;
      sample?: EventSample;
    }[];
  };
  frame_correlation: {
    total_frame_count: number;
    truncated: boolean;
    frames: CorrelatedFrameSummary[];
  };
  frames?: { kind?: string; frame_number?: number; cycle?: number }[];
  dispatch?: {
    serial_ordered: boolean;
    synced_event_count: number;
    unsynced_event_count: number;
    dispatched_event_count: number;
    gap_count: number;
    missing_serial_count: number;
    sync_count: number;
    gaps?: { after_serial: number; missing_count: number; kind: string }[];
  };
  session?: SessionInfo;
};

export type UtraceDashboard = {
  schema_version: number;
  status: InspectStatus;
  path: string;
  dashboard: TraceDashboardBody;
};

export type UtraceDecodePhase = "reading" | "analyzing" | "complete";

/** Metadata for the CPU range index retained by a browser trace session. */
export type CpuTimelineIndexInfo = {
  source_bytes: number;
  source_fingerprint: number;
  cycle_frequency?: number;
  total_interval_count: number;
  indexed_interval_count: number;
  truncated: boolean;
  begin_cycle?: number;
  end_cycle?: number;
};

export type UtraceProgressEvent =
  | {
      protocol_version: 1;
      type: "bootstrap";
      sequence: number;
      progress: {
        phase: UtraceDecodePhase;
        bytes_consumed: number;
        total_bytes?: number;
        packets_observed?: number;
      };
      bootstrap?: {
        header: TraceDashboardBody["header"];
        prologue?: TraceDashboardBody["prologue"];
        thread_info: NonNullable<TraceDashboardBody["thread_info"]>;
        declared_event_types: number;
        packets: Record<string, unknown>;
        thread_info_truncated: boolean;
      };
    }
  | {
      protocol_version: 1;
      type: "snapshot";
      sequence: number;
      progress: {
        phase: UtraceDecodePhase;
        bytes_consumed: number;
        total_bytes?: number;
        packets_observed: number;
      };
      patch:
        | { type: "transport"; packets: Record<string, unknown> }
        | {
            type: "frames";
            total_frame_count: number;
            truncated: boolean;
            frames: {
              frame_number: number;
              frame_type: number;
              begin_cycle: number;
              end_cycle: number;
              duration_cycles: number;
              duration_seconds?: number;
              gpu_submitted_work_count?: number;
              gpu_submitted_work_cycles?: number;
            }[];
          };
    }
  | {
      protocol_version: 1;
      type: "complete";
      sequence: number;
      progress: {
        phase: "complete";
        bytes_consumed: number;
        total_bytes?: number;
        packets_observed?: number;
      };
      dashboard: UtraceDashboard;
      inventory?: UtraceInventory;
      timeline_index?: CpuTimelineIndexInfo;
    }
  | {
      protocol_version: 1;
      type: "failed";
      sequence: number;
      error: string;
    };

export type UtraceInventory = {
  schema_version: number;
  status: InspectStatus;
  path: string;
  inventory: {
    summary: {
      declared_event_types: number;
      observed_event_types: number;
      observed_events: number;
      decoded_event_types: number;
      partial_event_types: number;
      raw_event_types: number;
    };
    events: {
      logger: string;
      event: string;
      observed_count: number;
      decode_status: "decoded" | "partial" | "raw";
    }[];
  };
};

export type ParseErrorBody = {
  error: string;
  exit_code?: number;
  stderr?: string;
  stdout?: string;
};
