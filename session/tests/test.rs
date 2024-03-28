use constructor::{Get, New, Set};
use ezsql::crud;

#[derive(Debug, Default, Get, Set, New)]
#[crud(table_name = "xxx", alias_fields = "xx:xxx,aa:bb", field_name_to_snake = true,
funs = [
{fn_name = "create_user1", sql_type = "create:single", fields = "name, age"},
{fn_name = "create_user2", sql_type = "create:batch", fields = "name, age", exist_update = "true"},
{fn_name = "delete_user1", sql_type = "delete", condition = "name:=, age:<=,id:="}
{fn_name = "delete_user2", sql_type = "delete"},
{fn_name = "update_user1", sql_type = "update", fields = "name, age", condition = "name:>, age:=<,id:="},
{fn_name = "update_user2", sql_type = "update", fields = "name, age", condition = "name:>, age:=<,id:="},
{fn_name = "read_user1", sql_type = "read:single", pre_where_sql = "select count(1)"},
{fn_name = "read_user2", sql_type = "read:single", fields = "name, age", condition = "name:=, age:>=,id:=", order = "name:DESC,age:ASC", res_type = "false"},
{fn_name = "read_user3", sql_type = "read:batch", fields = "name, age", condition = "name:=, age:>=,id:=", page = "true", order = "name:DESC,age:ASC", res_type = "true"},
{fn_name = "read_user4", sql_type = "read:batch", fields = "name, age", condition = "name:=, age:>=,id:=", order = "name:DESC,age:ASC", res_type = "true"},
{fn_name = "read_user5", sql_type = "read:single", fields = "name, age", condition = "name:=, age:>=,id:=", order = "name:DESC,age:ASC", res_type = "true"},
{fn_name = "read_user6", sql_type = "read:single", condition = "name:=, age:>=,id:=", pre_where_sql = "select count(1)"}
])]
struct User {
    id: i32,
    name: String,
    age: i32,
    sex: bool,
}

impl User {}

#[test]
fn test() {
    // let user = User::new(1, "a".to_string(), 2, true);
    let user = User::default();
    println!("{user:?}");
}
