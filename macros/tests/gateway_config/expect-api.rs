use macros::gateway_config;

fn main() {
    gateway_config! {
        [
            NotApi {
                app_name: "/api",
                host: "localhost",
            }
        ]
    }
}
