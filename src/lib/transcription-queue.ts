export type FileJobStatus = "queued" | "running" | "completed" | "failed" | "cancelled";

export interface FileJob {
  id: string;
  path: string;
  diarize: boolean;
  prompt: string | null;
  profileId: string | null;
  status: FileJobStatus;
}

export interface FileJobOptions {
  diarize: boolean;
  prompt: string | null;
  profileId: string | null;
}

export function createFileJobs(
  paths: readonly string[],
  options: FileJobOptions,
  idFactory: () => string,
): FileJob[] {
  return paths
    .filter((path) => path.length > 0)
    .map((path) => ({
      id: idFactory(),
      path,
      diarize: options.diarize,
      prompt: options.prompt,
      profileId: options.profileId,
      status: "queued" as const,
    }));
}

export function nextQueuedFile(jobs: readonly FileJob[], busy: boolean): FileJob | null {
  if (busy || jobs.some((job) => job.status === "running")) return null;
  return jobs.find((job) => job.status === "queued") ?? null;
}

export function updateFileJob(
  jobs: readonly FileJob[],
  id: string,
  status: FileJobStatus,
): FileJob[] {
  return jobs.map((job) => (job.id === id ? { ...job, status } : job));
}

export function fileJobName(path: string): string {
  const withoutTrailingSeparators = path.replace(/[\\/]+$/, "");
  const separator = Math.max(
    withoutTrailingSeparators.lastIndexOf("/"),
    withoutTrailingSeparators.lastIndexOf("\\"),
  );
  return withoutTrailingSeparators.slice(separator + 1);
}
