use actix_web::{Responder, Either, web};
use serde::Serialize;

struct AppState{
    db:timetrax::database::Database<'static, chrono::Utc>
}

#[derive(Serialize,Debug,Clone)]
struct WorkItem{
    id:u64,
    title:String
}

#[actix_web::get("/work_items")]
async fn get_work_items(data: actix_web::web::Data<AppState>) -> impl Responder {
    let work_items=&data.db.get_available_work();
    if let Ok(items)=work_items{
        let items:Vec<_>=items.iter().cloned().map(|(title,id)| WorkItem{id, title}).collect();
        let res=web::Json(items);
        Either::Left(res)
    } else {
        Either::Right(actix_web::HttpResponse::InternalServerError())
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let port = 8080;
    actix_web::HttpServer::new(move || {
        let db = timetrax::database::Database::open("work.db", &chrono::Utc).unwrap();
        let app_state=AppState{db};
        let api = actix_web::web::scope("/api").service(get_work_items);
        actix_web::App::new()
            .app_data(actix_web::web::Data::new(app_state))
            .service(api)
            .service(actix_files::Files::new("/static", "./static"))
    })
    .bind(("127.0.0.1", port))?
    .run()
    .await
}
