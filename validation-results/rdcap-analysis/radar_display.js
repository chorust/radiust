/* global VueMap */

$(function () {
    VueContent = new Vue({
        el: '#container',
        data: {
            no_data: false, //無資料時顯示遮布
            db_data: {
                //from DB
                elevation_angle: [], //三層觀測場-仰角資料
                parameter: [], //三層觀測場-參數資料
            },
            selected: {
                country: null,
                radar: null,
                datetime: null,
                duration: MapSetting.timebar.duration,
                type: null,
                observation_field: {
                    elevation_angle: '',
                    parameter: '',
                },
                single_wind: {
                    height: 1,
                },
                timestamp: null,
            },
            kmap_options: {},
            render_data: {
                header: null,
                list: [],
            },
            timebar: {
                color: MapSetting.timebar.color,
                duration: {
                    default: MapSetting.timebar.duration,
                },
                default_index: 0,
                list: [],
            },
            is_loading: false,
        },
        components: {},
        beforeCreate: function () {},
        computed: {
            has_wind_single: function () {
                if (this.selected.country === 'TWN' && this.selected.radar !== 'TWN') {
                    return true
                }
                return false
            },
            timebar_list: function () {
                return this.render_data.list.map((o) => {
                    return Number(o.key)
                })
            },
            play_data: function () {
                let obj = {}
                this.render_data.list.forEach((o) => {
                    obj[o.key] = o
                })

                return obj
            },
        },
        mounted: function () {
            this.selected.country = global.country
            this.selected.radar = global.radar
            this.selected.datetime = global.datetime

            this.get_radar_list()
            this.get_observ_field_options()

            this.selected.type = 'radar'
        },
        methods: {
            check_delay_play: function () {
                // 檢查產品是否已經載完
                // 1. 所有產品全下載完畢=> return false => 播放下一時間
                // 2. 還有產品沒載完=> return true => 等待
                return this.is_loading
            },
            change_timebar: function (data) {
                this.selected.timestamp = data.t
            },
            get_radar_list: function () {
                $.ajax({
                    url: global.base_url + 'data_access/get_radar_list',
                    type: 'post',
                    dataType: 'json',
                    data: {
                        radar_name: [this.selected.radar],
                    },
                    success: (data) => {
                        const latlng = [Number(data[0].Latitude), Number(data[0].Longitude)]
                        const zoom =
                            this.selected.country === 'ASIA'
                                ? MapSetting.zoom[this.selected.radar]
                                : MapSetting.zoom['radar']

                        this.$refs['kmap'].set_map_view({
                            latlng: latlng,
                            zoom: zoom,
                        })

                        if (this.selected.country !== 'ASIA') this.show_radar_markers(latlng)
                    },
                })
            },
            get_observ_field_options: function () {
                $.ajax({
                    url: global.base_url + 'data_access/get_observ_field_options',
                    type: 'POST',
                    dataType: 'json',
                    data: {
                        radar_name: this.selected.radar,
                    },
                    success: (data) => {
                        if (!data.elevation_angle.length || !data.parameter.length) return

                        this.db_data.elevation_angle = data.elevation_angle
                        this.db_data.parameter = data.parameter

                        // 預設選取第一個值
                        this.selected.observation_field.elevation_angle = 1
                        this.selected.observation_field.parameter = data.parameter[0]['value']
                    },
                })
            },
            get_radar_data: function () {
                $.ajax({
                    url: global.base_url + 'data_access/get_radar_data',
                    type: 'POST',
                    dataType: 'json',
                    data: {
                        country: this.selected.country,
                        radar_name: this.selected.radar,
                        datetime: this.selected.datetime,
                    },
                    success: (data) => {
                        this.no_data = false
                        this.render_data.header = data.header
                        this.render_data.list = data.list
                    },
                    error: (msg) => {
                        this.no_data = true
                        this.render_data.header = null
                        this.render_data.list = []
                    },
                })
            },
            get_radar_observ_data: function () {
                $.ajax({
                    url: global.base_url + 'data_access/get_radar_observ_data',
                    type: 'POST',
                    dataType: 'json',
                    data: {
                        radar_name: this.selected.radar,
                        layer: this.selected.observation_field.elevation_angle,
                        header_type: this.selected.observation_field.parameter,
                    },
                    success: (data) => {
                        this.no_data = false
                        this.render_data.header = data.header
                        this.render_data.list = data.list
                    },
                    error: (msg) => {
                        this.no_data = true
                        this.render_data.header = null
                        this.render_data.list = []
                    },
                })
            },
            get_single_wind_data: function () {
                $.ajax({
                    url: global.base_url + 'data_access/get_single_wind_data',
                    type: 'POST',
                    dataType: 'json',
                    data: {
                        radar_name: this.selected.radar,
                        layer: this.selected.single_wind.height,
                    },
                    success: (data) => {
                        this.no_data = false
                        this.render_data.header = data.header
                        this.render_data.list = data.list
                    },
                    error: (msg) => {
                        this.no_data = true
                        this.render_data.header = null
                        this.render_data.list = []
                    },
                })
            },
            show_radar_markers: function (latlng) {
                const pane_name = 'all_markers'
                this.$refs['kmap'].create_layer({
                    name: pane_name,
                    type: 'empty',
                    pane_type: 'top',
                    pane_ctrl: false,
                    callback: () => {
                        const marker_icon = this.$refs['kmap'].create_icon({
                            size: 'small', //marker大小 small|medium|large
                            bg_color: MapSetting.marker.background_color, //marker顏色
                            icon: MapSetting.marker.icon, //icon樣式
                        })
                        const marker = L.marker(latlng, {
                            icon: marker_icon,
                            pane: pane_name,
                            keyboard: false,
                        })
                        let html = '<table class="popup_table">'
                        html += '<tr><td>Short Name</td><td>' + this.selected.radar + '</td></tr>'
                        html += '<tr><td>Latitude</td><td>' + latlng[0] + '</td></tr>'
                        html += '<tr><td>Longitude</td><td>' + latlng[1] + '</td></tr>'
                        html += '</table>'

                        marker
                            .bindTooltip(html, { offset: [0, -25], maxWidth: '30em' })
                            .addTo(this.$refs['kmap'].panes[pane_name])
                    },
                    destory: () => {},
                })
            },
            show_render_product: function (timestamp) {
                this.is_loading = true

                const pane_name = 'radar_display'
                const pane = this.$refs['kmap'].panes[pane_name]

                if (!pane || !pane.render) return this.initial_render(timestamp)

                //render 產品已經 initial
                if (pane.render.is_exist(timestamp)) {
                    //切換資料
                    pane.render.change(timestamp)
                    this.is_loading = false
                } else {
                    //添加資料
                    pane.render.add_data({
                        data: this.play_data[timestamp],
                        callback: () => {
                            pane.render.change(timestamp)
                            this.is_loading = false
                        },
                    })
                }
            },
            initial_render: function (timestamp) {
                const pane_name = 'radar_display'
                const colorbar =
                    this.selected.type === 'observation_field'
                        ? MapSetting.colorbar[this.selected.observation_field.parameter]
                        : MapSetting.colorbar[this.selected.type]
                const sub_type_list = [null, 'pixel', 'symbol', 'multi']
                const sub_type = sub_type_list[this.play_data[timestamp]['url'].length]

                this.$refs['kmap'].create_layer({
                    name: pane_name,
                    type: 'render',
                    text: lang['product_' + this.selected.type],
                    data: {
                        header: this.render_data.header,
                        list: [this.play_data[timestamp]],
                    },
                    render_hint: {
                        sub_type: sub_type,
                        colorbar: colorbar,
                        //[for multi]
                        color_csv: 'spd',
                        multi_drawer: function (headerInfo, transform) {
                            //定義multi箭頭, blockSize,render為KXD核心指定名稱, 勿更名
                            this.blockSize = 14 //單一箭頭所占大小(含四周空白)

                            this.render = function (cnv, val) {
                                const ctx = cnv.getContext('2d')
                                const theta = (3 * Math.PI) / 2 // 數學上的零度為向右，逆時針旋轉為正
                                const arrowLen = transform.others[0](val.others[0]) * 0.9 + 0.5
                                const centerXY = this.blockSize / 2

                                let theta_tmp
                                // 符號中心點
                                const ptrX1 = centerXY
                                const ptrY1 = centerXY
                                // 符號右側尾
                                theta_tmp = theta - (5 * Math.PI) / 6
                                const ptrX2 = ptrX1 + (Math.cos(theta_tmp) * arrowLen) / 3
                                const ptrY2 = ptrY1 - (Math.sin(theta_tmp) * arrowLen) / 3
                                // 符號頂點
                                const ptrX3 = ptrX1 + (Math.cos(theta) * arrowLen) / 6
                                const ptrY3 = ptrY1 - (Math.sin(theta) * arrowLen) / 6
                                // 符號左側尾
                                theta_tmp = theta + (5 * Math.PI) / 6
                                const ptrX4 = ptrX1 + (Math.cos(theta_tmp) * arrowLen) / 3
                                const ptrY4 = ptrY1 - (Math.sin(theta_tmp) * arrowLen) / 3
                                // 符號尾巴
                                const ptrX5 = ptrX1 - (Math.cos(theta) * arrowLen) / 1
                                const ptrY5 = ptrY1 + (Math.sin(theta) * arrowLen) / 0.8

                                ctx.beginPath()

                                //箭頭
                                ctx.moveTo(ptrX2, ptrY2)
                                ctx.lineTo(ptrX3, ptrY3)
                                ctx.lineTo(ptrX4, ptrY4)
                                //箭頭線
                                ctx.moveTo(ptrX1, ptrY1)
                                ctx.lineTo(ptrX5, ptrY5)

                                ctx.stroke()
                                ctx.fill()
                            }
                        },
                    },
                    callback: () => {
                        this.is_loading = false
                    },
                    destory: () => {},
                })
            },
            reset: function () {
                this.selected.timestamp = null //避免舊產品時間相同, 無法觸發
                this.$refs['kmap'].clear_pane('radar_display')
            },
        },
        watch: {
            'selected.duration': function (duration) {
                this.$refs['timebar'].duration = duration
            },
            'selected.type': function (type) {
                this.reset()

                if (type === 'radar') this.get_radar_data()
                else if (type === 'observation_field') this.get_radar_observ_data()
                else if (type === 'wind_single') this.get_single_wind_data()
            },
            'selected.observation_field.elevation_angle': function () {
                if (this.selected.type !== 'observation_field') return

                this.reset()
                this.get_radar_observ_data()
            },
            'selected.observation_field.parameter': function () {
                if (this.selected.type !== 'observation_field') return

                this.reset()
                this.get_radar_observ_data()
            },
            'selected.single_wind.height': function () {
                if (this.selected.type !== 'wind_single') return

                this.reset()
                this.get_single_wind_data()
            },
            'render_data.list': function (list) {
                this.timebar.list = this.timebar_list

                const last_timestamp = this.timebar_list[list.length - 1]

                if (last_timestamp) {
                    this.$refs['timebar'].change(last_timestamp, 'timestamp')
                    this.selected.timestamp = last_timestamp //避免舊產品時間相同, 無法觸發
                }
            },
            'selected.timestamp': function (t) {
                if (!t) return

                this.show_render_product(t)
            },
        },
    })
})
