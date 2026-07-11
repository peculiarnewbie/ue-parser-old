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

export type CorrelatedFrameSummary = {
  frame_number: number;
  cpu_metadata_count: number;
  cpu_metadata_cycles: number;
  cpu_metadata_seconds?: number;
  top_cpu_scopes?: CpuScopeSummary[];
  gpu_queue_count: number;
  gpu_work_count: number;
  gpu_work_cycles: number;
  gpu_breadcrumb_count: number;
  gpu_breadcrumb_cycles: number;
  top_gpu_breadcrumbs?: { name: string; total_cycles: number }[];
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

export type UtraceDashboard = {
  schema_version: number;
  status: InspectStatus;
  path: string;
  dashboard: {
    header: { magic: string; transport: number; protocol: number };
    prologue?: { cycle_frequency?: number };
    thread_info?: { thread_id: number; name?: string }[];
    cpu: {
      scopes: CpuScopeSummary[];
      threads: {
        thread_id: number;
        name?: string;
        count: number;
        total_cycles: number;
        total_seconds?: number;
      }[];
      timeline?: CpuTimelineDashboard;
    };
    gpu: {
      total_frame_count: number;
      frames_truncated: boolean;
      frames: {
        frame_number: number;
        work_total_cycles: number;
        breadcrumb_total_cycles: number;
      }[];
      timeline?: GpuTimelineDashboard;
    };
    frame_correlation: {
      total_frame_count: number;
      truncated: boolean;
      frames: CorrelatedFrameSummary[];
    };
    counters: { specs?: unknown[]; samples?: unknown[] };
    memory?: Record<string, unknown>;
  };
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
