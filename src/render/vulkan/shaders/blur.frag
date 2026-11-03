#version 450
layout(location = 0) in vec2 v_texcoord;
layout(location = 0) out vec4 out_color;
layout(set = 0, binding = 0) uniform sampler2D u_texture;
layout(std140, set = 0, binding = 1) uniform Blur {
    vec4 transform_c0;
    vec4 transform_c1;
    vec4 transform_c2;
    vec4 tex_c0;
    vec4 tex_c1;
    vec4 tex_c2;
    vec2 item_size;
    vec2 direction;
    vec2 texel_size;
    float radius;
} draw;
void main() {
    float radius = max(draw.radius, 1.0);
    vec2 offset1 = draw.direction * draw.texel_size * (1.384615 * radius);
    vec2 offset2 = draw.direction * draw.texel_size * (3.230769 * radius);
    vec4 color = texture(u_texture, v_texcoord) * 0.227027;
    color += texture(u_texture, v_texcoord + offset1) * 0.316216;
    color += texture(u_texture, v_texcoord - offset1) * 0.316216;
    color += texture(u_texture, v_texcoord + offset2) * 0.070270;
    color += texture(u_texture, v_texcoord - offset2) * 0.070270;
    out_color = color;
}
