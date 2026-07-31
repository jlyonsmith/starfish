#![allow(unused_imports)]

include!("msgs.rs");

use tokio_postgres::types::ToSql;

impl UserUpdate {
    pub fn gen_update_sql<'a>(&'a self) -> Result<(String, Vec<&'a (dyn ToSql + Sync)>), String> {
        let mut index: usize = 0;
        let mut set_clauses = Vec::new();
        let mut params: Vec<&(dyn ToSql + Sync)> = Vec::new();

        // Where clause params
        params.insert(index, &self.alias);
        index += 1;

        // Set clause params
        for (column, value) in [
            ("email", &self.email),
            ("first_name", &self.first_name),
            ("last_name", &self.last_name),
        ] {
            if let Some(value) = value {
                params.push(value);
                set_clauses.push(format!("{} = ${}", column, index));
                index += 1;
            }
        }

        if set_clauses.is_empty() {
            return Err("no fields to update".to_string());
        }

        let sql = format!(
            "UPDATE users SET {} WHERE alias = ${} RETURNING id, alias, email, first_name, last_name, created_at, modified_at",
            set_clauses.join(", "),
            index
        );

        Ok((sql, params))
    }
}
