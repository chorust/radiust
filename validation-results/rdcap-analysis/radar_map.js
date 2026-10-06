/* global VueMap */

$(function () {
    VueContent = new Vue({
        el: '#container',
        data: {
            country_data: [], //國家資料 from DB
            display_config: [], //popup 顯示欄位 from DB
            radar_data: [], //all radar from DB(排除mosaic)
            markers: {}, //radar marker
            kmap: {
                panes: {}, // for kxd-map, 可控制pane
            },
            selected_country: null,
            selected_radar: null,
            markers_pane: 'all_markers',
        },
        components: {},
        beforeCreate: function () {
            if (global.global) {
                //for bundle
                lang = global.lang
                global = global.global
            }
        },
        computed: {
            country_map: function () {
                let obj = {}

                this.country_data.forEach((o) => {
                    obj[o.Country] = o.Country_key
                })

                return obj
            },
            country_menu: function () {
                //產左邊menu
                return this.country_data
                    .map((o) => {
                        return {
                            key: o.Country_key,
                            text: o.Country,
                            img_url: global.base_url + 'image/country/' + o.Country_key + '.png',
                        }
                    })
                    .filter((o) => {
                        //排除ASIA && 沒有雷達的國家
                        return o.key !== 'ASIA' && this.radar_list[o.key]
                    })
            },
            country_list: function () {
                return this.country_data.map((o) => {
                    return o.Country_key
                })
            },
            country_latlng: function () {
                let obj = {}
                this.country_data.forEach((o) => {
                    obj[o.Country_key] = {
                        latlng: [Number(o.Lat), Number(o.Lng)],
                        zoom: Number(o.Zoom),
                    }
                })

                return obj
            },
            radar_list: function () {
                let obj = {}
                this.radar_data.forEach((o) => {
                    const country_key = this.country_map[o.Country]
                    if (!obj[country_key]) {
                        obj[country_key] = []
                    }

                    obj[country_key].push(o.Short_name)
                })

                return obj
            },
            popup_data: function () {
                //markers popup html
                let obj = {}
                this.radar_data.forEach((radar) => {
                    //所有雷達的marker
                    const country_key = this.country_map[radar.Country]
                    const link = global.base_url + 'data_access/radar_display/' + country_key + '/' + radar.Short_name

                    let html = '<table class="popup_table">'
                    this.display_config.forEach((o) => {
                        html +=
                            '<tr>' +
                            '<td>' +
                            o.Display_text +
                            '</td>' +
                            '<td>' +
                            radar[o.Column_name] +
                            o.Unit +
                            '</td>' +
                            '</tr>'
                    })
                    html += '<tr>'
                    html += '<td>Display</td>'
                    html +=
                        '<td><a href="' +
                        link +
                        '" class="el-btn el-primary-fill" tabindex="0" target="_blank">' +
                        radar.Name +
                        '(' +
                        radar.Short_name +
                        ')</a></td>'
                    html += '</tr>'
                    html += '</table>'

                    obj[radar.Short_name] = html
                })

                return obj
            },
        },
        mounted: function () {
            this.get_radar_display_config()
            this.get_country_list()
            this.get_radar_list()
        },
        methods: {
            get_radar_display_config: function () {
                $.ajax({
                    url: global.base_url + 'data_access/get_radar_display_config',
                    dataType: 'json',
                    success: (data) => {
                        this.display_config = data
                    },
                })
            },
            get_country_list: function () {
                $.ajax({
                    url: global.base_url + 'data_access/get_country_list',
                    dataType: 'json',
                    success: (data) => {
                        this.country_data = data

                        this.is_loaded_ctrl()
                    },
                })
            },
            get_radar_list: function () {
                $.ajax({
                    url: global.base_url + 'data_access/get_radar_list',
                    dataType: 'json',
                    success: (data) => {
                        this.radar_data = data.filter((o) => {
                            return o.Country !== 'Asia'
                        })

                        this.is_loaded_ctrl()
                    },
                })
            },
            add_marker: function (options, open_popup = false) {
                const pane_name = this.markers_pane
                const marker_icon = this.$refs['kmap'].create_icon({
                    size: 'small', //marker大小 small|medium|large
                    color: options.color, //icon顏色
                    bg_color: options.background_color, //marker顏色
                    icon: options.icon_name, //icon樣式
                })
                const marker = L.marker([options.lat, options.lng], {
                    icon: marker_icon,
                    pane: pane_name,
                    keyboard: false,
                    zIndexOffset: open_popup ? 1000 : 0,
                })
                marker
                    .bindPopup(options.popup, {
                        offset: [0, -25],
                        maxWidth: '30em',
                    })
                    .addTo(this.$refs['kmap'].panes[pane_name])

                let timer
                marker
                    .on('mouseover', function () {
                        //0.5秒自動開啟popup
                        timer = setTimeout(function () {
                            marker.openPopup()
                        }, 500)
                    })
                    .on('mouseout', function () {
                        clearTimeout(timer)
                    })
                    .on('click', function () {
                        clearTimeout(timer)
                    })

                if (open_popup) {
                    marker.openPopup()
                }
            },
            is_loaded_ctrl: function () {
                if (!this.radar_data.length) {
                    return false
                }
                if (!this.country_data.length) {
                    return false
                }

                this.selected_country = 'TWN'
                return true
            },
        },
        watch: {
            selected_country: function (country) {
                this.selected_radar = country
                this.$refs['kmap'].set_map_view(this.country_latlng[country])
            },
            selected_radar: function (selected) {
                //所選雷達樣式
                const selected_is_country = this.country_list.indexOf(selected) >= 0

                this.$refs['kmap'].create_layer({
                    name: this.markers_pane,
                    type: 'empty',
                    text: 'Radar markers',
                    pane_type: 'top',
                    callback: () => {
                        this.radar_data.forEach((o) => {
                            //所有雷達的marker
                            let open_popup = false
                            let is_selected = o.Short_name === selected
                            if (selected_is_country) {
                                const country_key = this.country_map[o.Country]
                                is_selected = country_key === selected
                            } else if (is_selected) {
                                open_popup = true
                                this.$refs['kmap'].set_map_view({
                                    latlng: [o.Latitude, o.Longitude],
                                    zoom: 7,
                                })
                            }

                            this.add_marker(
                                {
                                    lat: o.Latitude,
                                    lng: o.Longitude,
                                    background_color:
                                        o.Status === 'Active' ? MapSetting.marker.background_color : '#909399',
                                    color: is_selected ? '#153352' : '#d9ecff',
                                    icon_name: is_selected ? 'circle' : MapSetting.marker.icon,
                                    popup: this.popup_data[o.Short_name],
                                },
                                open_popup
                            )
                        })
                    },
                    destory: () => {},
                })
            },
        },
    })
})
