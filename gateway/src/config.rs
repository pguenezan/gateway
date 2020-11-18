gateway_config! {
    [
        Api {
            app_name: "/foo_bar",
            host: "127.0.0.1:8000",
            mode: "forward_all",
        },
        Api {
            app_name: "/misc",
            host: "127.0.0.1:8001",
            mode: "forward_strict",
            endpoints: [
                // Endpoint {
                //     path: "/i_shoud_exist/",
                //     method: "POST",
                //     chain_to: [
                //         "/foo_bar/no_forward_all_please",
                //         "/misc/i_shoud_exist",
                //         "/misc/this_shoud_be_post",
                //     ],
                // },
                Endpoint {
                    path: "/this_shoud_be_post/",
                    method: "GET",
                },
                Endpoint {
                    path: "/report.{format}/",
                    method: "GET",
                },
                Endpoint {
                    path: "/report/",
                    method: "GET",
                },
                Endpoint {
                    path: "/report.{format}/view/",
                    method: "GET",
                },
                Endpoint {
                    path: "/report.{format}/edit/{user}/mail/",
                    method: "GET",
                },
                Endpoint {
                    path: "/report.{format}/edit/{user}/name/",
                    method: "GET",
                },
                Endpoint {
                    path: "/report./",
                    method: "GET",
                },
                Endpoint {
                    path: "/user/",
                    method: "GET",
                },
                Endpoint {
                    path: "/user/delete_all/yes/",
                    method: "GET",
                },
                Endpoint {
                    path: "/user/{id}/",
                    method: "GET",
                },
                Endpoint {
                    path: "/alone/{i_am}/",
                    method: "GET",
                },
            ],
        },
    ]
}
