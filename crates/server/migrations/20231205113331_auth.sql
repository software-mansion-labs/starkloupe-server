-- Rename 'team' table to 'projects'
ALTER TABLE
  "team" RENAME TO "projects";

-- Rename 'team_id' column to 'project_id' in 'simulations' table
ALTER TABLE
  "simulations" RENAME COLUMN "team_id" TO "project_id";