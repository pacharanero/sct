## `data-volume` folder

If you are developing inside the devcontainer, place your data files in the `data-volume` folder
— such as the SNOMED CT zip file and output files from sct (NDJSON, SQLite, Parquet, etc.).

The `data-volume` folder is backed by a Docker volume, so disk operations here will be significantly 
faster than using the root or other bind-mounted folders.