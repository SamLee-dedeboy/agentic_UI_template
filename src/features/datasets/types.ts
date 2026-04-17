export interface ColumnInfo {
  name: string;
  dtype: string;
}

export interface UploadedDataset {
  dataset_id: string;
  filename: string;
  format: "csv" | "json";
  row_count: number;
  columns: ColumnInfo[];
  sample_rows: Array<Record<string, unknown>>;
}
