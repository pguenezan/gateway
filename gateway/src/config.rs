gateway_config! {
    [
        Api {
            app_name: "/chartis",
            host: "127.0.0.1:8000",
            forward_path: "/chartis",
            mode: "forward_strict",
            endpoints: [
                Endpoint {
                    path: "/layer/{layerSlug}/mvt/{geoType}/",
                    method: "GET",
                },
            ],
        },
        Api {
            app_name: "/portal",
            host: "portal-configured-master-portal-configured:80",
            forward_path: "",
            mode: "forward_all",
        },
    ]
}

