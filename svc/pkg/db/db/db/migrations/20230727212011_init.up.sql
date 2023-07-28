CREATE TABLE databases (
	database_id UUID PRIMARY KEY,
	owner_team_id UUID NOT NULL,  -- References db-team.teams
	name_id STRING NOT NULL,
	create_ts INT NOT NULL,
	schema BYTES NOT NULL,  -- rivet.backend.db.Schema
	UNIQUE INDEX (owner_team_id, name_id ASC)
);

