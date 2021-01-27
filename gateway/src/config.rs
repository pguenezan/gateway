gateway_config! {
    [
        Api {
            app_name: "/phrit",
            host: "phrit-prod-phrit:80",
            forward_path: "/phrit",
            mode: "forward_strict",
            endpoints: [
                Endpoint {
                    path: "/values/",
                    method: "GET",
                },
                Endpoint {
                    path: "/{output}/{source}/ligne/",
                    method: "GET",
                },
                Endpoint {
                    path: "/{output}/{source}/pointremarquable/",
                    method: "GET",
                },
                Endpoint {
                    path: "/events/{id}/prediction/feedbacks/",
                    method: "POST",
                },
                Endpoint {
                    path: "/ligne/",
                    method: "GET",
                },
                Endpoint {
                    path: "/pointremarquable/eic/",
                    method: "GET",
                },
                Endpoint {
                    path: "/events/",
                    method: "GET",
                },
                Endpoint {
                    path: "/events/{id}/prediction/",
                    method: "GET",
                },
                Endpoint {
                    path: "/events/{id}/prediction/feedbacks/{feedback_id}/",
                    method: "GET",
                },
                Endpoint {
                    path: "/events/{id}/prediction/feedbacks/",
                    method: "GET",
                },
                Endpoint {
                    path: "/{output}/{source}/pointremarquable/{z}/{x}/{y}/",
                    method: "GET",
                },
                Endpoint {
                    path: "/{output}/{source}/ligne/{z}/{x}/{y}/",
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

