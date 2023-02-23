
```sql
CREATE TABLE IF NOT EXISTS "track_record" (
	"session_id"	varchar(36),
	"data_type"	varchar(10),
	"start_time"	INTEGER,
	"end_time"	INTEGER,
	"header" TEXT,
	"payload"	BLOB
);
CREATE INDEX IF NOT EXISTS "session_inx" ON "track_record" (
	"session_id"
);
```

